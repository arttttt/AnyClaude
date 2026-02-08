use crate::ui::layout::centered_rect_by_size;
use crate::ui::theme::{CLAUDE_ORANGE, POPUP_BORDER};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Reusable centered popup dialog with an orange title and bordered frame.
pub struct PopupDialog<'a> {
    title: &'a str,
    lines: Vec<Line<'a>>,
    min_width: u16,
    fixed_width: Option<u16>,
}

impl<'a> PopupDialog<'a> {
    pub fn new(title: &'a str, lines: Vec<Line<'a>>) -> Self {
        Self {
            title,
            lines,
            min_width: 0,
            fixed_width: None,
        }
    }

    pub fn min_width(mut self, w: u16) -> Self {
        self.min_width = w;
        self
    }

    pub fn fixed_width(mut self, w: u16) -> Self {
        self.fixed_width = Some(w);
        self
    }

    /// Render the dialog and return the occupied `Rect` (useful for overlays like scrollbars).
    pub fn render(self, frame: &mut Frame, area: Rect) -> Rect {
        let popup_width = match self.fixed_width {
            Some(w) => w,
            None => {
                let content_width =
                    self.lines.iter().map(Line::width).max().unwrap_or(0) as u16;
                content_width.saturating_add(4).max(self.min_width)
            }
        };
        let popup_height = (self.lines.len() as u16).saturating_add(2);
        let rect = centered_rect_by_size(area, popup_width, popup_height);

        frame.render_widget(Clear, rect);
        let title = Line::from(vec![
            Span::styled("─", Style::default().fg(POPUP_BORDER)),
            Span::styled(format!(" {} ", self.title), Style::default().fg(CLAUDE_ORANGE)),
        ]);
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(POPUP_BORDER));
        let widget = Paragraph::new(self.lines).block(block);
        frame.render_widget(widget, rect);
        rect
    }
}
