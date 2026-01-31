use crate::ui::app::{App, PopupKind};
use crate::ui::footer::Footer;
use crate::ui::header::Header;
use crate::ui::layout::{centered_rect_by_size, layout_regions};
use crate::ui::terminal::TerminalBody;
use crate::ui::theme::{ACTIVE_HIGHLIGHT, CLAUDE_ORANGE, HEADER_TEXT, POPUP_BORDER, STATUS_ERROR};
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
                if let Some(status) = app.proxy_status() {
                    lines.push(Line::from(format!(
                        "Active backend: {}",
                        status.active_backend
                    )));
                    lines.push(Line::from(format!("Uptime: {}s", status.uptime_seconds)));
                    lines.push(Line::from(format!(
                        "Total requests: {}",
                        status.total_requests
                    )));
                    lines.push(Line::from(format!("Healthy: {}", status.healthy)));
                } else {
                    lines.push(Line::from("Status pending..."));
                }

                if let Some(metrics) = app.metrics() {
                    if let Some(active) = app.proxy_status().map(|s| s.active_backend.as_str()) {
                        if let Some(backend) = metrics.per_backend.get(active) {
                            lines.push(Line::from(""));
                            lines.push(Line::from(format!(
                                "Latency p50/p95/p99: {:?}/{:?}/{:?} ms",
                                backend.p50_latency_ms,
                                backend.p95_latency_ms,
                                backend.p99_latency_ms
                            )));
                            lines.push(Line::from(format!(
                                "Avg latency: {:.1} ms",
                                backend.avg_latency_ms
                            )));
                            lines.push(Line::from(format!(
                                "Avg TTFB: {:.1} ms",
                                backend.avg_ttfb_ms
                            )));
                            lines.push(Line::from(format!("Timeouts: {}", backend.timeouts)));
                        }
                    }
                }

                if let Some(error) = app.last_ipc_error() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(format!("IPC error: {error}")));
                }

                ("Status", lines)
            }
            PopupKind::BackendSwitch => {
                let mut lines = Vec::new();
                if app.backends().is_empty() {
                    lines.push(Line::from("No backends available."));
                } else {
                    for (idx, backend) in app.backends().iter().enumerate() {
                        let marker = if backend.is_active { "●" } else { "○" };
                        let mut spans = Vec::new();
                        spans.push(Span::styled(
                            format!("{:>2}. ", idx + 1),
                            Style::default().fg(HEADER_TEXT),
                        ));
                        spans.push(Span::styled(marker, Style::default().fg(HEADER_TEXT)));
                        spans.push(Span::raw(" "));
                        spans.push(Span::styled(
                            &backend.display_name,
                            Style::default().fg(HEADER_TEXT),
                        ));
                        if !backend.is_configured {
                            spans.push(Span::raw(" "));
                            spans.push(Span::styled(
                                "(missing credentials)",
                                Style::default().fg(STATUS_ERROR),
                            ));
                        }
                        let mut line = Line::from(spans);
                        if backend.is_active {
                            line = line.style(Style::default().bg(ACTIVE_HIGHLIGHT));
                        }
                        lines.push(line);
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::from("Press a number to switch."));
                }

                if let Some(error) = app.last_ipc_error() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(format!("IPC error: {error}")));
                }

                ("Switch Backend", lines)
            }
        };

        let content_width = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
        let popup_width = content_width.saturating_add(4);
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
