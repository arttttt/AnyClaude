//! `VtEmulator` wires the parser to the grid.
//!
//! Owns a `Parser`, a `Grid`, the running window title, and an output
//! response buffer for replies the host must write back to the PTY (DA,
//! DSR). The DEC mode / OSC integration layer lives in a separate
//! commit; this one ships the core dispatch.

use crate::grid::{CursorStyle, Grid, MouseMode, Row};
use crate::parser::{Action, EraseMode, Parser, SgrAction};
use crate::{CellFlags, TermColor};

/// Cursor state surfaced to the renderer.
#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
    pub style: CursorStyle,
}

/// Snapshot of the rendered state taken at one point in time. Clones
/// the visible rows so the renderer can hold the data across frames
/// without taking a long-lived borrow on the emulator.
pub struct RenderSnapshot {
    pub rows: Vec<Row>,
    pub cursor: CursorState,
    pub title: String,
    pub cwd: Option<String>,
}

/// Public terminal-emulator interface. Wraps the parser+grid so callers
/// don't have to know about either.
pub trait TerminalEmulator: Send {
    /// Feed raw bytes from the PTY through the parser.
    fn process(&mut self, bytes: &[u8]);

    /// Resize the visible grid (columns and rows in cells, not pixels).
    fn resize(&mut self, cols: usize, rows: usize);

    /// Snapshot for rendering. Cheap-ish (clones visible rows only).
    fn snapshot(&self) -> RenderSnapshot;

    /// Take and clear the pending PTY response buffer (DA, DSR, focus
    /// notifications, …). The caller writes the returned bytes to the PTY.
    fn take_responses(&mut self) -> Vec<u8>;

    fn mouse_mode(&self) -> MouseMode;
    fn bracketed_paste(&self) -> bool;
    fn cursor_keys_app(&self) -> bool;
    fn focus_reporting(&self) -> bool;
    fn title(&self) -> &str;
}

pub struct VtEmulator {
    parser: Parser,
    grid: Grid,
    title: String,
    cwd: Option<String>,
    response_buf: Vec<u8>,
}

impl VtEmulator {
    pub fn new(cols: usize, rows: usize, max_scrollback: usize) -> Self {
        Self {
            parser: Parser::new(),
            grid: Grid::new(cols, rows, max_scrollback),
            title: String::new(),
            cwd: None,
            response_buf: Vec::new(),
        }
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    fn apply_action(&mut self, action: Action) {
        match action {
            // Printable / C0
            Action::Print(c) => self.grid.print(c),
            Action::Bell => { /* visual bell hook would go here */ }
            Action::Backspace => self.grid.cursor_back(1),
            Action::Tab => self.grid.next_tab(1),
            Action::LineFeed | Action::LineFeedAlt => self.grid.linefeed(),
            Action::CarriageReturn => self.grid.carriage_return(),
            Action::AbortSequence => { /* sequence already cleared by parser */ }

            // Cursor moves
            Action::CursorUp(n) => self.grid.cursor_up(n as usize),
            Action::CursorDown(n) => self.grid.cursor_down(n as usize),
            Action::CursorForward(n) => self.grid.cursor_forward(n as usize),
            Action::CursorBack(n) => self.grid.cursor_back(n as usize),
            Action::CursorNextLine(n) => self.grid.cursor_next_line(n as usize),
            Action::CursorPrevLine(n) => self.grid.cursor_prev_line(n as usize),
            Action::CursorColumn(c) => self.grid.cursor_column(c as usize),
            Action::CursorVerticalAbs(r) => self.grid.cursor_vertical(r as usize),
            Action::CursorPosition { row, col } => {
                self.grid.cursor_position(row as usize, col as usize);
            }
            Action::CursorTab(n) => self.grid.next_tab(n as usize),
            Action::CursorBackTab(n) => self.grid.prev_tab(n as usize),

            // Edit
            Action::EraseDisplay(mode) => self.grid.erase_display(mode),
            Action::EraseLine(mode) => self.grid.erase_line(mode),
            Action::EraseChars(n) => self.grid.erase_chars(n as usize),
            Action::InsertChars(n) => self.grid.insert_chars(n as usize),
            Action::DeleteChars(n) => self.grid.delete_chars(n as usize),
            Action::InsertLines(n) => self.grid.insert_lines(n as usize),
            Action::DeleteLines(n) => self.grid.delete_lines(n as usize),
            Action::RepeatLast(n) => self.grid.repeat_last(n as usize),

            // Scroll
            Action::ScrollUp(n) => self.grid.scroll_up(n as usize),
            Action::ScrollDown(n) => self.grid.scroll_down(n as usize),
            Action::SetScrollRegion { top, bottom } => {
                self.grid.set_scroll_region(top, bottom);
            }

            // SGR
            Action::SetAttr(sgr) => self.apply_sgr(sgr),

            // DEC modes — minimal handling here; full integration in commit 7.
            Action::DecModeSet(mode) => self.set_dec_mode(mode, true),
            Action::DecModeReset(mode) => self.set_dec_mode(mode, false),
            Action::RequestMode(_) => { /* DECRQM reply lands in commit 7 */ }

            // Device replies
            Action::DeviceStatusReport(6) => {
                // CPR: report cursor pos (1-based). DECOM-aware? VT100 standard:
                // report origin-relative when DECOM is on. We report absolute
                // for simplicity; can be revisited if real apps care.
                let reply = format!(
                    "\x1b[{};{}R",
                    self.grid.cursor_row + 1,
                    self.grid.cursor_col + 1,
                );
                self.response_buf.extend_from_slice(reply.as_bytes());
            }
            Action::DeviceStatusReport(5) => {
                // Status report OK.
                self.response_buf.extend_from_slice(b"\x1b[0n");
            }
            Action::DeviceStatusReport(_) => {}
            Action::DeviceAttributes => {
                // Primary DA — claim VT102 (1;6). vte/Warp also use this.
                self.response_buf.extend_from_slice(b"\x1b[?6c");
            }

            // Cursor save/restore
            Action::SaveCursor => self.grid.save_cursor(),
            Action::RestoreCursor => self.grid.restore_cursor(),
            Action::SaveCursorSco => self.grid.save_cursor_sco(),
            Action::RestoreCursorSco => self.grid.restore_cursor_sco(),

            // Simple ESC
            Action::Index => self.grid.linefeed(),
            Action::NextLine => {
                self.grid.carriage_return();
                self.grid.linefeed();
            }
            Action::ReverseIndex => self.grid.reverse_index(),
            Action::KeypadAppMode(on) => self.grid.keypad_app = on,
            Action::FullReset => {
                self.grid.reset();
                self.title.clear();
                self.cwd = None;
            }
            Action::SetCursorStyle(n) => {
                self.grid.cursor_style = CursorStyle::from_decscusr(n);
            }
            Action::SetTabStop => { /* fixed tab=8; HTS ignored */ }
            Action::TabClear(_) => { /* fixed tab=8; TBC ignored */ }

            // OSC (basics; full integration lands in commit 7)
            Action::SetTitle(t) => self.title = t,
            Action::SetCwd(_) | Action::Hyperlink { .. } | Action::PromptMarker(_) => {
                // Wired in commit 7.
            }

            Action::Unsupported => {}
        }
    }

    fn apply_sgr(&mut self, sgr: SgrAction) {
        match sgr {
            SgrAction::Reset => {
                self.grid.current_fg = TermColor::Default;
                self.grid.current_bg = TermColor::Default;
                self.grid.current_flags = CellFlags::empty();
            }
            SgrAction::SetFlag(flag) => self.grid.current_flags.set(flag),
            SgrAction::ClearFlag(flag) => self.grid.current_flags.clear(flag),
            SgrAction::Foreground(c) => self.grid.current_fg = c,
            SgrAction::Background(c) => self.grid.current_bg = c,
            SgrAction::DefaultForeground => self.grid.current_fg = TermColor::Default,
            SgrAction::DefaultBackground => self.grid.current_bg = TermColor::Default,
        }
    }

    /// Minimal DEC private mode handler — the common ones needed for the
    /// core test surface. Full coverage (mouse, focus, sync output, paste)
    /// lands in commit 7.
    fn set_dec_mode(&mut self, mode: u16, enable: bool) {
        match mode {
            1 => self.grid.cursor_keys_app = enable,
            6 => {
                self.grid.origin_mode = enable;
                self.grid.cursor_position(1, 1);
            }
            7 => self.grid.auto_wrap = enable,
            25 => self.grid.cursor_visible = enable,
            47 | 1047 => {
                if enable {
                    self.grid.enter_alt_screen();
                } else {
                    self.grid.exit_alt_screen();
                }
            }
            1049 => {
                if enable {
                    self.grid.enter_alt_screen();
                    self.grid.erase_display(EraseMode::All);
                } else {
                    self.grid.exit_alt_screen();
                }
            }
            _ => { /* commit 7 covers the rest */ }
        }
    }
}

impl TerminalEmulator for VtEmulator {
    fn process(&mut self, bytes: &[u8]) {
        let mut actions = Vec::with_capacity(bytes.len() / 4);
        self.parser.advance(bytes, |a| actions.push(a));
        for action in actions {
            self.apply_action(action);
        }
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
    }

    fn snapshot(&self) -> RenderSnapshot {
        RenderSnapshot {
            rows: self.grid.visible_iter().cloned().collect(),
            cursor: CursorState {
                row: self.grid.cursor_row,
                col: self.grid.cursor_col,
                visible: self.grid.cursor_visible,
                style: self.grid.cursor_style,
            },
            title: self.title.clone(),
            cwd: self.cwd.clone(),
        }
    }

    fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.response_buf)
    }

    fn mouse_mode(&self) -> MouseMode {
        self.grid.mouse_mode
    }
    fn bracketed_paste(&self) -> bool {
        self.grid.bracketed_paste
    }
    fn cursor_keys_app(&self) -> bool {
        self.grid.cursor_keys_app
    }
    fn focus_reporting(&self) -> bool {
        self.grid.focus_reporting
    }
    fn title(&self) -> &str {
        &self.title
    }
}
