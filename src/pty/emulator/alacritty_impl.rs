use super::{CursorState, TermCell, TermColor, TerminalEmulator};
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Timeout};
use std::time::Duration;

struct NoOpListener;

impl EventListener for NoOpListener {
    fn send_event(&self, _event: alacritty_terminal::event::Event) {}
}

/// No-op timeout that disables synchronized output buffering.
///
/// The sync protocol (CSI ?2026h/l) is designed for direct terminal I/O to
/// prevent flickering during redraws.  We do in-memory emulation with our own
/// buffered rendering via ratatui, so sync mode only adds latency and causes
/// stale cursor state between buffer flushes.
#[derive(Default)]
struct NoSyncTimeout;

impl Timeout for NoSyncTimeout {
    fn set_timeout(&mut self, _duration: Duration) {}
    fn clear_timeout(&mut self) {}
    fn pending_timeout(&self) -> bool {
        false
    }
}

/// Terminal dimensions for alacritty's `Dimensions` trait.
struct TermSize {
    lines: usize,
    cols: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub(super) struct AlacrittyEmulator {
    term: alacritty_terminal::Term<NoOpListener>,
    processor: alacritty_terminal::vte::ansi::Processor<NoSyncTimeout>,
}

impl AlacrittyEmulator {
    pub(super) fn new(rows: u16, cols: u16, scrollback_len: usize) -> Self {
        let config = Config {
            scrolling_history: scrollback_len,
            ..Config::default()
        };

        let size = TermSize {
            lines: rows as usize,
            cols: cols as usize,
        };

        let term = alacritty_terminal::Term::new(config, &size, NoOpListener);
        let processor = alacritty_terminal::vte::ansi::Processor::default();

        Self { term, processor }
    }
}

impl TerminalEmulator for AlacrittyEmulator {
    fn process(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    fn set_size(&mut self, rows: u16, cols: u16) {
        let size = TermSize {
            lines: rows as usize,
            cols: cols as usize,
        };
        self.term.resize(size);
    }

    fn cell(&self, row: u16, col: u16) -> Option<TermCell> {
        let grid = self.term.grid();
        let display_offset = grid.display_offset();
        let line = alacritty_terminal::index::Line(row as i32 - display_offset as i32);
        let column = alacritty_terminal::index::Column(col as usize);

        // Bounds check
        if row as usize >= grid.screen_lines() || col as usize >= grid.columns() {
            return None;
        }

        let cell = &grid[line][column];

        let is_wide_continuation = cell.flags.contains(Flags::WIDE_CHAR_SPACER);
        let has_contents = cell.c != ' ' && cell.c != '\0';

        let symbol = if cell.c == '\0' {
            String::new()
        } else {
            let mut s = String::with_capacity(4);
            s.push(cell.c);
            if let Some(zerowidth) = cell.zerowidth() {
                for &zw in zerowidth {
                    s.push(zw);
                }
            }
            s
        };

        Some(TermCell {
            symbol,
            fg: convert_color(cell.fg),
            bg: convert_color(cell.bg),
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
            inverse: cell.flags.contains(Flags::INVERSE),
            has_contents,
            is_wide_continuation,
        })
    }

    fn scrollback(&self) -> usize {
        self.term.grid().display_offset()
    }

    fn set_scrollback(&mut self, offset: usize) {
        use alacritty_terminal::grid::Scroll;

        let current = self.term.grid().display_offset();
        if offset == current {
            return;
        }
        let delta = offset as i32 - current as i32;
        self.term.scroll_display(Scroll::Delta(delta));
    }

    fn cursor(&self) -> CursorState {
        let point = self.term.grid().cursor.point;
        let visible = self.term.mode().contains(TermMode::SHOW_CURSOR);

        CursorState {
            row: point.line.0 as u16,
            col: point.column.0 as u16,
            visible,
        }
    }

    fn mouse_tracking(&self) -> bool {
        let mode = self.term.mode();
        mode.contains(TermMode::MOUSE_MODE) || mode.contains(TermMode::MOUSE_MOTION)
    }
}

fn convert_color(color: Color) -> TermColor {
    match color {
        Color::Named(NamedColor::Foreground) | Color::Named(NamedColor::Background) => {
            TermColor::Default
        }
        Color::Named(name) => TermColor::Indexed(name as u8),
        Color::Indexed(idx) => TermColor::Indexed(idx),
        Color::Spec(rgb) => TermColor::Rgb(rgb.r, rgb.g, rgb.b),
    }
}
