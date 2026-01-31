use crate::ui::app::{App, PopupKind};
use crate::ui::footer::Footer;
use crate::ui::header::Header;
use crate::ui::layout::{centered_rect_by_size, layout_regions};
use crate::ui::terminal::TerminalBody;
use crate::ui::theme::{
    ACTIVE_HIGHLIGHT, CLAUDE_ORANGE, HEADER_TEXT, POPUP_BORDER, STATUS_ERROR, STATUS_OK,
};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::sync::Arc;
use termwiz::surface::CursorVisibility;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.size();
    let (header, body, footer) = layout_regions(area);

    let header_widget = Header::new();
    frame.render_widget(header_widget.widget(app.proxy_status()), header);
    frame.render_widget(Clear, body);
    if let Some(screen) = app.screen() {
        frame.render_widget(TerminalBody::new(Arc::clone(&screen)), body);
        if app.focus_is_terminal() && body.width > 0 && body.height > 0 {
            if let Ok(screen) = screen.lock() {
                if screen.cursor_visibility() == CursorVisibility::Visible {
                    let (x, y) = screen.cursor_position();
                    let x = body.x + x.min(body.width.saturating_sub(1) as usize) as u16;
                    let y = body.y + y.min(body.height.saturating_sub(1) as usize) as u16;
                    frame.set_cursor(x, y);
                }
            }
        }
    }
    let footer_widget = Footer::new();
    frame.render_widget(footer_widget.widget(), footer);

    if let Some(kind) = app.popup_kind() {
        let (title, lines) = match kind {
            PopupKind::Status => {
                let mut lines = Vec::new();

                // Find active backend info
                let active_backend = app
                    .backends()
                    .iter()
                    .find(|b| b.is_active);

                if let Some(backend) = active_backend {
                    // Provider
                    lines.push(Line::from(vec![
                        Span::styled("  Provider:  ", Style::default().fg(HEADER_TEXT)),
                        Span::styled(&backend.display_name, Style::default().fg(HEADER_TEXT)),
                    ]));

                    // Model
                    let model = backend.model_hint.as_deref().unwrap_or("unknown");
                    lines.push(Line::from(vec![
                        Span::styled("  Model:     ", Style::default().fg(HEADER_TEXT)),
                        Span::styled(model, Style::default().fg(HEADER_TEXT)),
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
                    let (status_text, status_color) = if app.proxy_status().is_some_and(|s| s.healthy) {
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

                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("  Esc/Ctrl+S: Close", Style::default().fg(HEADER_TEXT)),
                ]));

                if let Some(error) = app.last_ipc_error() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::styled(format!("  IPC error: {error}"), Style::default().fg(STATUS_ERROR)),
                    ]));
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
        };

        let content_width = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
        let popup_min_width = match kind {
            PopupKind::BackendSwitch => 60,
            _ => 0,
        };
        let popup_width = content_width.saturating_add(4).max(popup_min_width);
        let popup_height = lines.len().saturating_add(2) as u16;
        let area = centered_rect_by_size(body, popup_width, popup_height);

        frame.render_widget(Clear, area);
        let popup = Block::default()
            .title(Span::styled(title, Style::default().fg(CLAUDE_ORANGE)))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(POPUP_BORDER));
        let widget = Paragraph::new(lines).block(popup);
        frame.render_widget(widget, area);
    }
}
