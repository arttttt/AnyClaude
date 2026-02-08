use crate::ui::layout::centered_rect_by_size;
use crate::ui::theme::{CLAUDE_ORANGE, HEADER_SEPARATOR, HEADER_TEXT, POPUP_BORDER};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Reusable centered popup dialog with an orange title and bordered frame.
pub struct PopupDialog<'a> {
    title: &'a str,
    lines: Vec<Line<'a>>,
    footer: Option<&'a str>,
    min_width: u16,
    fixed_width: Option<u16>,
    /// (total_items, scroll_offset) — enables scrollbar when set.
    scrollbar: Option<(usize, usize)>,
}

impl<'a> PopupDialog<'a> {
    pub fn new(title: &'a str, lines: Vec<Line<'a>>) -> Self {
        Self {
            title,
            lines,
            footer: None,
            min_width: 0,
            fixed_width: None,
            scrollbar: None,
        }
    }

    pub fn footer(mut self, text: &'a str) -> Self {
        self.footer = Some(text);
        self
    }

    pub fn min_width(mut self, w: u16) -> Self {
        self.min_width = w;
        self
    }

    pub fn fixed_width(mut self, w: u16) -> Self {
        self.fixed_width = Some(w);
        self
    }

    /// Enable scrollbar. `total_items` is the full count, `scroll_offset` is current position.
    pub fn scrollbar(mut self, total_items: usize, scroll_offset: usize) -> Self {
        self.scrollbar = Some((total_items, scroll_offset));
        self
    }

    /// Render the dialog and return the occupied `Rect` (useful for overlays like scrollbars).
    pub fn render(mut self, frame: &mut Frame, area: Rect) -> Rect {
        let content_rows = self.lines.len();

        // Append footer as centered line with a separator
        if let Some(text) = self.footer {
            self.lines.push(Line::from(""));
            self.lines.push(
                Line::from(Span::styled(text, Style::default().fg(HEADER_TEXT)))
                    .centered(),
            );
        }

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

        // Manual scrollbar — ratatui's Scrollbar rounds start/end independently,
        // causing ±1 thumb size jitter. Draw manually for constant thumb size.
        if let Some((total_items, scroll_offset)) = self.scrollbar {
            let visible = content_rows;
            if total_items > visible && visible > 0 {
                let max_offset = total_items.saturating_sub(visible);
                let track = visible;
                let thumb_size = (track * visible / total_items).max(1);
                let thumb_start = if max_offset > 0 {
                    scroll_offset * (track - thumb_size) / max_offset
                } else {
                    0
                };

                let x = rect.x + rect.width - 2; // adjacent to border
                let y_base = rect.y + 1; // skip top border
                let buf = frame.buffer_mut();
                for i in 0..track {
                    let cell = &mut buf[(x, y_base + i as u16)];
                    if i >= thumb_start && i < thumb_start + thumb_size {
                        cell.set_char('┃');
                        cell.set_style(Style::default().fg(HEADER_SEPARATOR));
                    }
                }
            }
        }

        rect
    }
}
