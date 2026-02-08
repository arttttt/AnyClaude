use crate::error::ErrorSeverity;
use crate::ui::app::{App, PopupKind};
use crate::ui::footer::Footer;
use crate::ui::header::Header;
use crate::ui::history::render_history_dialog;
use crate::ui::layout::layout_regions;
use crate::ui::components::PopupDialog;
use crate::ui::terminal::TerminalBody;
use crate::ui::theme::{
    ACTIVE_HIGHLIGHT, HEADER_TEXT, STATUS_ERROR, STATUS_OK, STATUS_WARNING,
};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;
use ratatui::Frame;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let (header, body, footer) = layout_regions(area);

    let header_widget = Header::new();
    frame.render_widget(
        header_widget.widget(app.proxy_status(), app.error_registry()),
        header,
    );
    frame.render_widget(Clear, body);
    if let Some(parser) = app.parser() {
        frame.render_widget(TerminalBody::new(Arc::clone(&parser)), body);
        // Only show cursor when in live view (scrollback == 0) and terminal has focus
        if app.focus_is_terminal() && app.scrollback() == 0 && body.width > 0 && body.height > 0 {
            let parser_guard = parser.lock();
            let screen = parser_guard.screen();
            if !screen.hide_cursor() {
                let cursor = screen.cursor_position();
                let x = body.x + cursor.1.min(body.width.saturating_sub(1));
                let y = body.y + cursor.0.min(body.height.saturating_sub(1));
                frame.set_cursor_position((x, y));
            }
        }
    }
    let footer_widget = Footer::new();
    frame.render_widget(footer_widget.widget(footer), footer);

    if let Some(kind) = app.popup_kind() {
        // History dialog renders itself independently
        if matches!(kind, PopupKind::History) {
            render_history_dialog(frame, app.history_dialog());
            return;
        }

        let (title, lines) = match kind {
            PopupKind::Status => {
                let mut lines = Vec::new();

                // Find active backend info
                let active_backend = app.backends().iter().find(|b| b.is_active);

                if let Some(backend) = active_backend {
                    // Provider
                    lines.push(Line::from(vec![
                        Span::styled("  Provider:  ", Style::default().fg(HEADER_TEXT)),
                        Span::styled(&backend.display_name, Style::default().fg(HEADER_TEXT)),
                    ]));

                    // URL (truncate if too long)
                    let url = if backend.base_url.len() > 40 {
                        format!("{}...", &backend.base_url[..37])
                    } else {
                        backend.base_url.clone()
                    };
                    lines.push(Line::from(vec![
                        Span::styled("  URL:       ", Style::default().fg(HEADER_TEXT)),
                        Span::styled(url, Style::default().fg(HEADER_TEXT)),
                    ]));

                    // Status
                    let (status_text, status_color) =
                        if app.proxy_status().is_some_and(|s| s.healthy) {
                            ("Connected", STATUS_OK)
                        } else {
                            ("Error", STATUS_ERROR)
                        };
                    lines.push(Line::from(vec![
                        Span::styled("  Status:    ", Style::default().fg(HEADER_TEXT)),
                        Span::styled(status_text, Style::default().fg(status_color)),
                        Span::styled("   ●", Style::default().fg(status_color)),
                    ]));

                    // Latency - get from most recent request for this backend
                    let latency_str = if let Some(metrics) = app.metrics() {
                        metrics
                            .recent
                            .iter()
                            .rev()
                            .find(|r| r.backend == backend.id)
                            .and_then(|r| r.latency_ms)
                            .map(|ms| format!("{} ms", ms))
                            .unwrap_or_else(|| "—".to_string())
                    } else {
                        "—".to_string()
                    };
                    lines.push(Line::from(vec![
                        Span::styled("  Latency:   ", Style::default().fg(HEADER_TEXT)),
                        Span::styled(latency_str, Style::default().fg(HEADER_TEXT)),
                    ]));

                    // Tokens - estimated from most recent request
                    let tokens_str = if let Some(metrics) = app.metrics() {
                        metrics
                            .recent
                            .iter()
                            .rev()
                            .find(|r| r.backend == backend.id)
                            .map(|r| {
                                let input = r
                                    .request_analysis
                                    .as_ref()
                                    .and_then(|a| a.estimated_input_tokens)
                                    .map(|t| format!("{}", t))
                                    .unwrap_or_else(|| "—".to_string());
                                // Estimate output tokens from response bytes (rough: ~4 chars per token)
                                let output = if r.response_bytes > 0 {
                                    format!("{}", r.response_bytes / 4)
                                } else {
                                    "—".to_string()
                                };
                                format!("{} in / {} out", input, output)
                            })
                            .unwrap_or_else(|| "— in / — out".to_string())
                    } else {
                        "— in / — out".to_string()
                    };
                    lines.push(Line::from(vec![
                        Span::styled("  Tokens:    ", Style::default().fg(HEADER_TEXT)),
                        Span::styled(tokens_str, Style::default().fg(HEADER_TEXT)),
                    ]));

                } else {
                    lines.push(Line::from("  No backend configured"));
                }

                // Show recent errors from error registry
                let recent_errors: Vec<_> = app
                    .error_registry()
                    .all_errors()
                    .into_iter()
                    .rev()
                    .filter(|e| e.severity >= ErrorSeverity::Warning)
                    .take(5)
                    .collect();

                if !recent_errors.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![Span::styled(
                        "  Recent Issues:",
                        Style::default().fg(STATUS_WARNING),
                    )]));

                    for error in recent_errors {
                        let time_ago = format_time_ago(error.timestamp);
                        let color = match error.severity {
                            ErrorSeverity::Critical | ErrorSeverity::Error => STATUS_ERROR,
                            ErrorSeverity::Warning => STATUS_WARNING,
                            ErrorSeverity::Info => STATUS_OK,
                        };
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("    [{time_ago}] "),
                                Style::default().fg(HEADER_TEXT),
                            ),
                            Span::styled(error.message.clone(), Style::default().fg(color)),
                        ]));

                        if let Some(details) = &error.details {
                            // Show first line of details
                            if let Some(first_line) = details.lines().next() {
                                let truncated = if first_line.len() > 50 {
                                    format!("{}...", &first_line[..47])
                                } else {
                                    first_line.to_string()
                                };
                                lines.push(Line::from(vec![Span::styled(
                                    format!("      {truncated}"),
                                    Style::default().fg(HEADER_TEXT),
                                )]));
                            }
                        }
                    }
                }

                // Show active recovery operations
                let recoveries = app.error_registry().active_recoveries();
                if !recoveries.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![Span::styled(
                        "  Recovery:",
                        Style::default().fg(STATUS_WARNING),
                    )]));

                    for recovery in &recoveries {
                        lines.push(Line::from(vec![Span::styled(
                            format!(
                                "    {} (attempt {}/{})",
                                recovery.operation, recovery.attempt, recovery.max_attempts
                            ),
                            Style::default().fg(STATUS_WARNING),
                        )]));
                    }
                }

                lines.push(Line::from(""));
                lines.push(Line::from(vec![Span::styled(
                    "  Esc/Ctrl+S: Close",
                    Style::default().fg(HEADER_TEXT),
                )]));

                if let Some(error) = app.last_ipc_error() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![Span::styled(
                        format!("  IPC error: {error}"),
                        Style::default().fg(STATUS_ERROR),
                    )]));
                }

                ("Network Diagnostics", lines)
            }
            PopupKind::BackendSwitch => {
                let mut lines = Vec::new();
                if app.backends().is_empty() {
                    lines.push(Line::from("    No backends available."));
                } else {
                    let selected_index = app.backend_selection();
                    let max_name_width = app
                        .backends()
                        .iter()
                        .map(|backend| backend.display_name.chars().count())
                        .max()
                        .unwrap_or(0);

                    for (idx, backend) in app.backends().iter().enumerate() {
                        let (status_text, status_color) = if backend.is_active {
                            ("Active", STATUS_OK)
                        } else if backend.is_configured {
                            ("Ready", STATUS_OK)
                        } else {
                            ("Missing", STATUS_ERROR)
                        };
                        let is_selected = idx == selected_index;

                        // Apply highlight background to all spans when selected
                        // (Line::style() doesn't propagate bg to pre-styled spans)
                        let base_style = if is_selected {
                            Style::default().bg(ACTIVE_HIGHLIGHT)
                        } else {
                            Style::default()
                        };

                        let mut spans = Vec::new();
                        let prefix = if is_selected {
                            format!("  → {}. ", idx + 1)
                        } else {
                            format!("    {}. ", idx + 1)
                        };
                        spans.push(Span::styled(prefix, base_style.fg(HEADER_TEXT)));
                        spans.push(Span::styled(
                            format!("{:<width$}", backend.display_name, width = max_name_width),
                            base_style.fg(HEADER_TEXT),
                        ));
                        spans.push(Span::styled("  [", base_style));
                        spans.push(Span::styled(status_text, base_style.fg(status_color)));
                        spans.push(Span::styled("]", base_style));

                        lines.push(Line::from(spans));
                    }

                    lines.push(Line::from(""));
                    lines.push(Line::from(
                        "    Up/Down: Move  Enter: Select  Esc/Ctrl+B: Close",
                    ));
                }

                if let Some(error) = app.last_ipc_error() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(format!("    IPC error: {error}")));
                }

                ("Select Backend", lines)
            }
            PopupKind::History => unreachable!("handled above"),
        };

        let mut dialog = PopupDialog::new(title, lines);
        if matches!(kind, PopupKind::BackendSwitch) {
            dialog = dialog.min_width(60);
        }
        dialog.render(frame, body);

    }
}

/// Format a timestamp as a human-readable relative time.
fn format_time_ago(timestamp: SystemTime) -> String {
    let now = SystemTime::now();
    let elapsed = now.duration_since(timestamp).unwrap_or(Duration::ZERO);

    if elapsed.as_secs() < 60 {
        format!("{}s ago", elapsed.as_secs())
    } else if elapsed.as_secs() < 3600 {
        format!("{}m ago", elapsed.as_secs() / 60)
    } else {
        format!("{}h ago", elapsed.as_secs() / 3600)
    }
}
