use parking_lot::Mutex;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use std::sync::Arc;

pub struct TerminalBody {
    parser: Arc<Mutex<vt100::Parser>>,
}

impl TerminalBody {
    pub fn new(parser: Arc<Mutex<vt100::Parser>>) -> Self {
        Self { parser }
    }
}

impl Widget for TerminalBody {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let parser = self.parser.lock();

        let screen = parser.screen();
        let max_rows = area.height as usize;
        let max_cols = area.width as usize;

        for row_idx in 0..max_rows {
            let y = area.y + row_idx as u16;
            for col_idx in 0..max_cols {
                let x = area.x + col_idx as u16;
                let cell = screen.cell(row_idx as u16, col_idx as u16);
                if let Some(cell) = cell {
                    // Skip wide character continuation cells - the first cell already
                    // contains the full character and ratatui handles the width
                    if cell.is_wide_continuation() {
                        continue;
                    }

                    let style = style_from_cell(cell);

                    if let Some(cell_ref) = buf.cell_mut((x, y)) {
                        if cell.has_contents() {
                            // Cell has actual content - render it
                            cell_ref.set_symbol(cell.contents()).set_style(style);
                        } else if cell.bgcolor() != vt100::Color::Default {
                            // Empty cell but has background color - render space with style
                            cell_ref.set_symbol(" ").set_style(style);
                        }
                        // Otherwise leave cell as-is (already cleared by ratatui)
                    }
                }
            }
        }
    }
}

fn style_from_cell(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    // Foreground color
    if let Some(color) = color_from_vt100(cell.fgcolor()) {
        style = style.fg(color);
    }

    // Background color
    if let Some(color) = color_from_vt100(cell.bgcolor()) {
        style = style.bg(color);
    }

    // Text attributes
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }

    style
}

fn color_from_vt100(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(idx) => Some(Color::Indexed(idx)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}
