use crate::ui::theme::{GLOBAL_BORDER, HEADER_TEXT};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Footer;

impl Footer {
    pub fn new() -> Self {
        Self
    }

    pub fn widget(&self, area: Rect) -> Paragraph<'static> {
        let hints = " Ctrl+B: Switch │ Ctrl+S: Status │ Ctrl+H: History │ Ctrl+Q: Quit";
        let version = format!("v{} ", VERSION);

        // Calculate padding using char count, not byte count (for Unicode)
        let hints_width = hints.chars().count();
        let version_width = version.chars().count();
        let content_width = area.width.saturating_sub(2) as usize; // minus borders
        let padding = content_width
            .saturating_sub(hints_width)
            .saturating_sub(version_width);

        let text_style = Style::default().fg(HEADER_TEXT).add_modifier(Modifier::DIM);

        let line = Line::from(vec![
            Span::styled(hints, text_style),
            Span::styled(" ".repeat(padding), text_style),
            Span::styled(version, text_style),
        ]);

        Paragraph::new(line)
            .style(text_style)
            .alignment(Alignment::Left)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(GLOBAL_BORDER)),
            )
    }
}
