use crate::error::{ErrorRegistry, ErrorSeverity};
use crate::ipc::ProxyStatus;
use crate::ui::theme::{GLOBAL_BORDER, HEADER_TEXT, STATUS_ERROR, STATUS_OK, STATUS_WARNING};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

pub struct Header;

impl Header {
    pub fn new() -> Self {
        Self
    }

    pub fn widget(
        &self,
        status: Option<&ProxyStatus>,
        error_registry: &ErrorRegistry,
    ) -> Paragraph<'static> {
        let text_style = Style::default().fg(HEADER_TEXT).add_modifier(Modifier::DIM);

        // Determine status icon and color based on error registry
        let (icon, status_color, error_message) =
            if let Some(error) = error_registry.current_error() {
                match error.severity {
                    ErrorSeverity::Critical | ErrorSeverity::Error => {
                        ("🔴", STATUS_ERROR, Some(error.message.clone()))
                    }
                    ErrorSeverity::Warning => ("🟡", STATUS_WARNING, Some(error.message.clone())),
                    ErrorSeverity::Info => ("🟢", STATUS_OK, None),
                }
            } else if let Some(recovery) = error_registry.active_recoveries().first() {
                let msg = format!(
                    "Retrying... (attempt {}/{})",
                    recovery.attempt, recovery.max_attempts
                );
                ("🟡", STATUS_WARNING, Some(msg))
            } else {
                match status {
                    Some(s) if s.healthy => ("🟢", STATUS_OK, None),
                    Some(_) => ("🔴", STATUS_ERROR, Some("Connection error".to_string())),
                    None => ("⚪", STATUS_ERROR, None),
                }
            };

        let backend = status
            .map(|value| value.active_backend.as_str())
            .unwrap_or("unknown");
        let total_requests = status.map(|value| value.total_requests).unwrap_or(0);
        let uptime = status.map(|value| value.uptime_seconds).unwrap_or(0);
        let status_style = Style::default().fg(status_color);

        let mut spans = vec![
            Span::styled(" ", text_style),
            Span::styled(icon, status_style),
            Span::styled(" ", text_style),
        ];

        // Show error message in header if present
        if let Some(msg) = error_message {
            // Truncate message if too long
            let display_msg = if msg.len() > 40 {
                format!("{}...", &msg[..37])
            } else {
                msg
            };
            spans.push(Span::styled(display_msg, Style::default().fg(status_color)));
            spans.push(Span::styled(" │ ", text_style));
        }

        spans.extend([
            Span::styled(format!("Backend: {backend}"), text_style),
            Span::styled(" │ ", text_style),
            Span::styled(format!("Reqs: {total_requests}"), text_style),
            Span::styled(" │ ", text_style),
            Span::styled(format!("Uptime: {uptime}s"), text_style),
        ]);

        let line = Line::from(spans);

        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(GLOBAL_BORDER)),
        )
    }
}
