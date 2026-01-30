use crate::ipc::ProxyStatus;
use crate::ui::theme::{GLOBAL_BORDER, HEADER_SEPARATOR, HEADER_TEXT, STATUS_ERROR, STATUS_OK};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

pub struct Header;

impl Header {
    pub fn new() -> Self {
        Self
    }

    pub fn widget(&self, status: Option<&ProxyStatus>) -> Paragraph<'static> {
        let text_style = Style::default().fg(HEADER_TEXT);
        let separator_style = Style::default().fg(HEADER_SEPARATOR);
        let (icon, status_color) = match status {
            Some(status) if status.healthy => ("🟢", STATUS_OK),
            Some(_) => ("🔴", STATUS_ERROR),
            None => ("⚪", STATUS_ERROR),
        };
        let backend = status
            .map(|value| value.active_backend.as_str())
            .unwrap_or("unknown");
        let total_requests = status.map(|value| value.total_requests).unwrap_or(0);
        let uptime = status.map(|value| value.uptime_seconds).unwrap_or(0);
        let status_style = Style::default().fg(status_color);
        let line = Line::from(vec![
            Span::styled("  ", text_style),
            Span::styled(icon, status_style),
            Span::styled("  ", text_style),
            Span::styled(format!("Backend {backend}"), text_style),
            Span::styled("  │  ", separator_style),
            Span::styled(format!("Reqs {total_requests}"), text_style),
            Span::styled("  │  ", separator_style),
            Span::styled(format!("Uptime {uptime}s"), text_style),
        ]);

        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(Style::default().fg(GLOBAL_BORDER)),
        )
    }
}
