use crate::pty::emulator::TerminalEmulator;
use crate::pty::{TermCell, TermColor};
use parking_lot::Mutex;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use std::sync::Arc;

pub struct TerminalBody {
    emulator: Arc<Mutex<Box<dyn TerminalEmulator>>>,
}

impl TerminalBody {
    pub fn new(emulator: Arc<Mutex<Box<dyn TerminalEmulator>>>) -> Self {
        Self { emulator }
    }
}

impl Widget for TerminalBody {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let emu = self.emulator.lock();

        let max_rows = area.height as usize;
        let max_cols = area.width as usize;

        for row_idx in 0..max_rows {
            let y = area.y + row_idx as u16;
            for col_idx in 0..max_cols {
                let x = area.x + col_idx as u16;
                let Some(cell) = emu.cell(row_idx as u16, col_idx as u16) else {
                    continue;
                };

                // Skip wide character continuation cells - the first cell already
                // contains the full character and ratatui handles the width
                if cell.is_wide_continuation {
                    continue;
                }

                let style = style_from_cell(&cell);

                if let Some(cell_ref) = buf.cell_mut((x, y)) {
                    if cell.has_contents {
                        cell_ref.set_symbol(&cell.symbol).set_style(style);
                    } else if cell.bg != TermColor::Default || cell.inverse {
                        // Render styled space: background color or inverse video
                        // (e.g. cursor rendered as inverse space by child process)
                        cell_ref.set_symbol(" ").set_style(style);
                    }
                }
            }
        }
    }
}

fn style_from_cell(cell: &TermCell) -> Style {
    let mut style = Style::default();

    if let Some(color) = color_from_term(cell.fg) {
        style = style.fg(color);
    }

    if let Some(color) = color_from_term(cell.bg) {
        style = style.bg(color);
    }

    if cell.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse {
        style = style.add_modifier(Modifier::REVERSED);
    }

    style
}

fn color_from_term(color: TermColor) -> Option<Color> {
    match color {
        TermColor::Default => None,
        TermColor::Indexed(idx) => Some(Color::Indexed(idx)),
        TermColor::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}
