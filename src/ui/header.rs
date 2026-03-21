use crate::error::{ErrorRegistry, ErrorSeverity};
use crate::ipc::ProxyStatus;
use crate::ui::theme::{GLOBAL_BORDER, HEADER_TEXT, STATUS_ERROR, STATUS_OK, STATUS_WARNING};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

pub struct Header;

impl Default for Header {
    fn default() -> Self {
        Self::new()
    }
}

impl Header {
    pub fn new() -> Self {
        Self
    }

    /// Compute the column range (start, end) of the "Session: ..." text in the header.
    /// Columns are absolute (include left border offset).
    pub fn session_col_range(
        status: Option<&ProxyStatus>,
        error_registry: &ErrorRegistry,
        session_id: &str,
    ) -> (u16, u16) {
        let mut col: u16 = 1; // left border

        // " " + emoji(2 cells) + " "
        col += 1 + 2 + 1;

        // Optional error/recovery message + separator
        if let Some(error) = error_registry.current_error() {
            match error.severity {
                ErrorSeverity::Critical | ErrorSeverity::Error | ErrorSeverity::Warning => {
                    let msg_len = if error.message.len() > 40 { 40 } else { error.message.len() };
                    col += msg_len as u16 + 3; // message + " │ "
                }
                ErrorSeverity::Info => {}
            }
        } else if let Some(recovery) = error_registry.active_recoveries().first() {
            let msg = format!(
                "Retrying... (attempt {}/{})",
                recovery.attempt, recovery.max_attempts
            );
            col += msg.len() as u16 + 3;
        }

        let backend = status
            .map(|s| s.active_backend.as_str())
            .unwrap_or("unknown");
        let total_requests = status.map(|s| s.total_requests).unwrap_or(0);
        let uptime = status.map(|s| s.uptime_seconds).unwrap_or(0);

        col += format!("Backend: {backend}").len() as u16;
        col += 3; // " │ "
        col += format!("Reqs: {total_requests}").len() as u16;
        col += 3;
        col += format!("Uptime: {uptime}s").len() as u16;
        col += 3;

        let start = col;
        let end = col + format!("Session: {session_id}").len() as u16;
        (start, end)
    }

    pub fn widget(
        &self,
        status: Option<&ProxyStatus>,
        error_registry: &ErrorRegistry,
        session_id: &str,
        session_copied: bool,
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
            Span::styled(" │ ", text_style),
            if session_copied {
                Span::styled("Session ID copied!", Style::default().fg(STATUS_OK))
            } else {
                Span::styled(format!("Session: {session_id}"), text_style)
            },
        ]);

        let line = Line::from(spans);

        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(GLOBAL_BORDER)),
        )
    }
}
