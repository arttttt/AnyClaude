use crate::ui::theme::{GLOBAL_BORDER, HEADER_SEPARATOR, HEADER_TEXT, STATUS_OK};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

pub struct Header;

impl Header {
    pub fn new() -> Self {
        Self
    }

    pub fn widget(&self) -> Paragraph<'static> {
        let text_style = Style::default().fg(HEADER_TEXT);
        let separator_style = Style::default().fg(HEADER_SEPARATOR);
        let status_style = Style::default().fg(STATUS_OK);
        let line = Line::from(vec![
            Span::styled("🟢 ", status_style),
            Span::styled("Backend", text_style),
            Span::styled(" │ ", separator_style),
            Span::styled("model", text_style),
            Span::styled(" │ ", separator_style),
            Span::styled("tokens", text_style),
        ]);

        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(Style::default().fg(GLOBAL_BORDER)),
        )
    }
}
