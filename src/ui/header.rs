use crate::ipc::ProxyStatus;
use crate::ui::theme::{GLOBAL_BORDER, HEADER_TEXT, STATUS_OK};
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
        session_id: &str,
        subagent_backend_name: &str,
        teammate_backend_name: &str,
    ) -> (u16, u16) {
        let mut col: u16 = 1; // left border

        let backend = status
            .map(|s| s.active_backend.as_str())
            .unwrap_or("unknown");
        let total_requests = status.map(|s| s.total_requests).unwrap_or(0);
        let uptime = status.map(|s| s.uptime_seconds).unwrap_or(0);

        // " Backend: {name}"
        col += 1; // leading space
        col += format!("Backend: {backend}").len() as u16;
        col += 3; // " │ "
        col += format!("sub: {subagent_backend_name}").len() as u16;
        col += 3; // " │ "
        col += format!("team: {teammate_backend_name}").len() as u16;
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
        session_id: &str,
        session_copied: bool,
        subagent_backend_name: &str,
        teammate_backend_name: &str,
    ) -> Paragraph<'static> {
        let text_style = Style::default().fg(HEADER_TEXT).add_modifier(Modifier::DIM);

        let backend = status
            .map(|value| value.active_backend.as_str())
            .unwrap_or("unknown");
        let total_requests = status.map(|value| value.total_requests).unwrap_or(0);
        let uptime = status.map(|value| value.uptime_seconds).unwrap_or(0);

        let spans = vec![
            Span::styled(" ", text_style),
            Span::styled(format!("Backend: {backend}"), text_style),
            Span::styled(" │ ", text_style),
            Span::styled(format!("sub: {subagent_backend_name}"), text_style),
            Span::styled(" │ ", text_style),
            Span::styled(format!("team: {teammate_backend_name}"), text_style),
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
        ];

        let line = Line::from(spans);

        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(GLOBAL_BORDER)),
        )
    }
}
