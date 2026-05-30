//! Fixed-cell terminal grid: `Cell`, `Row`, `Grid` plus cursor state,
//! scroll region, alt screen, and the per-frame attributes used as a
//! template when printing new cells.
//!
//! Variable-width rendering is delegated to `term_gpu`. `term_core` is
//! logically monospace for VT correctness — `CUP row 5 col 10` always
//! addresses `Row[5].cells[10]`.

use crate::{CellFlags, TermColor};

/// One grid cell.
///
/// Layout target is small and clone-cheap; combining marks and other
/// rare metadata live behind `Box<CellExtra>` so the hot path stays
/// compact. For Claude Code's expected cell volume, the precise packing
/// is not load-bearing.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// Primary character. For wide characters this is on the left half;
    /// the right half is a spacer cell with `CellFlags::WIDE_CHAR_SPACER`.
    pub c: char,
    pub fg: TermColor,
    pub bg: TermColor,
    pub flags: CellFlags,
    /// Combining marks, OSC 8 hyperlinks, OSC 133 prompt markers. `None`
    /// on the common path.
    pub extra: Option<Box<CellExtra>>,
}

impl Cell {
    pub const fn space() -> Self {
        Self {
            c: ' ',
            fg: TermColor::Default,
            bg: TermColor::Default,
            flags: CellFlags::empty(),
            extra: None,
        }
    }

    /// Reset to a blank, default-attributes cell.
    pub fn reset(&mut self) {
        *self = Cell::space();
    }

    /// Append a zero-width / combining codepoint to this cell.
    /// Bounded by `MAX_ZEROWIDTH_BYTES` to defend against pathological
    /// input.
    pub fn push_zerowidth(&mut self, c: char) {
        let extra = self.extra.get_or_insert_with(Box::default);
        extra.push_zerowidth(c);
    }

    /// OSC 8 hyperlink target attached to this cell, if any.
    pub fn hyperlink(&self) -> Option<&str> {
        self.extra.as_ref().and_then(|e| e.hyperlink.as_deref())
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::space()
    }
}

/// Rare per-cell metadata, heap-allocated so the common `Cell` stays small.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CellExtra {
    /// Combining / zero-width codepoints stacked onto the base char.
    pub zerowidth: Vec<char>,
    /// OSC 8 hyperlink target, if set on this cell.
    pub hyperlink: Option<String>,
    /// OSC 133 prompt marker payload, if set.
    pub prompt: Option<PromptMarker>,
}

/// Soft cap on bytes stored in `CellExtra::zerowidth`.
/// Matches Warp's `Cell::push_zerowidth` warn-at-128, hard-at-256 policy.
const MAX_ZEROWIDTH_BYTES: usize = 256;

impl CellExtra {
    pub fn push_zerowidth(&mut self, c: char) {
        let used: usize = self.zerowidth.iter().map(|c| c.len_utf8()).sum();
        if used + c.len_utf8() <= MAX_ZEROWIDTH_BYTES {
            self.zerowidth.push(c);
        }
    }
}

/// OSC 133 prompt marker payload.
///
/// - `A` — start of prompt
/// - `B` — end of prompt (start of input region)
/// - `P` — continuation payload (key=value parameters)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptMarker {
    Start,
    End,
    Cont(String),
}

// ──── Row + Grid ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorStyle {
    #[default]
    BlockSteady,
    BlockBlink,
    UnderlineSteady,
    UnderlineBlink,
    BeamSteady,
    BeamBlink,
}

impl CursorStyle {
    /// Decode the DECSCUSR parameter (`CSI Ps SP q`).
    pub fn from_decscusr(n: u16) -> Self {
        match n {
            1 => Self::BlockBlink,
            2 => Self::BlockSteady,
            3 => Self::UnderlineBlink,
            4 => Self::UnderlineSteady,
            5 => Self::BeamBlink,
            6 => Self::BeamSteady,
            _ => Self::BlockSteady,
        }
    }
}

/// xterm mouse tracking level (mutually exclusive), set by DECSET
/// 1000 / 1002 / 1003. DECSET 9 (X10) is intentionally unsupported
/// (deprecated; Warp omits it too).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseTracking {
    #[default]
    Off,
    /// 1000 — report button press + release.
    Normal,
    /// 1002 — also report motion while a button is held.
    ButtonEvent,
    /// 1003 — report all pointer motion, button or not.
    AnyEvent,
}

/// Mouse-report byte encoding, set independently of the tracking level.
/// The UTF-8 (1005) and urxvt (1015) encodings are intentionally
/// unsupported (deprecated; SGR superseded them, and Warp omits them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    /// Legacy `CSI M Cb Cx Cy` (byte-offset 32, coords clamp at 223).
    #[default]
    Default,
    /// 1006 — SGR `CSI < Cb ; Cx ; Cy M|m` (no coordinate limit).
    Sgr,
}

/// The composite mouse-reporting protocol state. The tracking level and
/// the encoding are set by orthogonal DECSET sequences, so e.g. `1000`
/// + `1006` compose into click-tracking with SGR encoding rather than
/// clobbering each other (the bug a single conflated enum had).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseProtocol {
    pub tracking: MouseTracking,
    pub encoding: MouseEncoding,
}

impl MouseProtocol {
    /// Whether any tracking level is active (reports should be sent).
    pub fn is_active(&self) -> bool {
        !matches!(self.tracking, MouseTracking::Off)
    }

    /// Whether motion (drag / move) events are reported (1002 / 1003).
    pub fn reports_motion(&self) -> bool {
        matches!(
            self.tracking,
            MouseTracking::ButtonEvent | MouseTracking::AnyEvent
        )
    }

    /// Whether motion is reported even with no button held (1003).
    pub fn reports_bare_motion(&self) -> bool {
        matches!(self.tracking, MouseTracking::AnyEvent)
    }

    /// Whether reports use the SGR (1006) byte form rather than the legacy one.
    pub fn is_sgr(&self) -> bool {
        matches!(self.encoding, MouseEncoding::Sgr)
    }
}

/// A row of fixed-width cells.
#[derive(Debug, Clone)]
pub struct Row {
    pub cells: Vec<Cell>,
}

impl Row {
    pub fn new(cols: usize) -> Self {
        Self {
            cells: vec![Cell::space(); cols],
        }
    }

    pub fn resize(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::space());
    }

    /// Clear cells in `range` (in-place reset to blank).
    pub fn clear_range(&mut self, range: std::ops::Range<usize>) {
        let end = range.end.min(self.cells.len());
        let start = range.start.min(end);
        for cell in &mut self.cells[start..end] {
            cell.reset();
        }
    }
}

/// Main grid — visible rows plus scrollback. Column count is fixed
/// across scrollback and visible rows.
pub struct Grid {
    /// `[scrollback..., visible...]`. Visible region is the last
    /// `visible_rows` entries.
    rows: Vec<Row>,
    visible_rows: usize,
    cols: usize,
    max_scrollback: usize,
    /// Monotonic count of scrollback lines evicted off the top because the
    /// buffer was full. Lets a scrolled-up viewport stay anchored to its
    /// content as old lines erode (the analog of Warp's `num_lines_truncated`).
    lines_evicted: u64,

    // Cursor state
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
    pub cursor_style: CursorStyle,

    /// Last printed grapheme — used by `REP` (CSI b) to repeat.
    pub last_printed: Option<char>,

    /// Saved cursor (DECSC / DECRC)
    saved_cursor: Option<SavedCursor>,
    /// SCO save / restore (CSI s / CSI u) — distinct from DECSC.
    saved_cursor_sco: Option<(usize, usize)>,

    /// Scroll region (top, bottom) — 0-based inclusive.
    pub scroll_top: usize,
    pub scroll_bottom: usize,

    /// Current drawing attributes used to fill freshly printed cells.
    pub current_fg: TermColor,
    pub current_bg: TermColor,
    pub current_flags: CellFlags,

    /// Active OSC 8 hyperlink target `(params, url)`. Attached to every
    /// printed cell while set; an empty `url` clears it.
    pub current_hyperlink: Option<(String, String)>,
    /// One-shot OSC 133 prompt marker. Attached to the next printed cell
    /// and then cleared.
    pub next_prompt: Option<PromptMarker>,

    /// Alt screen state.
    alt_rows: Option<Vec<Row>>,
    alt_cursor: Option<(usize, usize)>,
    /// SGR template snapshot at the moment `enter_alt_screen` ran,
    /// restored by `exit_alt_screen`. Without this, attributes set
    /// in alt-screen rendering (BOLD, UNDERLINE, custom fg/bg) bled
    /// back into the primary buffer once Claude Code exited its
    /// welcome / TUI display.
    alt_sgr: Option<(TermColor, TermColor, CellFlags)>,

    /// Modes.
    pub origin_mode: bool,
    pub auto_wrap: bool,
    pub bracketed_paste: bool,
    pub focus_reporting: bool,
    pub sync_output: bool,
    pub mouse: MouseProtocol,
    pub cursor_keys_app: bool,
    pub keypad_app: bool,

    /// Scrollback offset in **pixels** — owned by term_gpu's scroll path
    /// but stored here so `RenderSnapshot` can carry it back to the
    /// renderer. Logical grid is fixed-cell; scroll is pixel-precise.
    pub scroll_offset_y: f32,
}

#[derive(Debug, Clone, Copy)]
struct SavedCursor {
    row: usize,
    col: usize,
    fg: TermColor,
    bg: TermColor,
    flags: CellFlags,
}

impl Grid {
    pub fn new(cols: usize, rows: usize, max_scrollback: usize) -> Self {
        let visible: Vec<Row> = (0..rows).map(|_| Row::new(cols)).collect();
        Self {
            rows: visible,
            visible_rows: rows,
            cols,
            max_scrollback,
            lines_evicted: 0,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            cursor_style: CursorStyle::BlockSteady,
            last_printed: None,
            saved_cursor: None,
            saved_cursor_sco: None,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            current_fg: TermColor::Default,
            current_bg: TermColor::Default,
            current_flags: CellFlags::empty(),
            current_hyperlink: None,
            next_prompt: None,
            alt_rows: None,
            alt_cursor: None,
            alt_sgr: None,
            origin_mode: false,
            auto_wrap: true,
            bracketed_paste: false,
            focus_reporting: false,
            sync_output: false,
            mouse: MouseProtocol::default(),
            cursor_keys_app: false,
            keypad_app: false,
            scroll_offset_y: 0.0,
        }
    }

    // ─── Inspectors ────────────────────────────────────────────────────────
    pub fn cols(&self) -> usize {
        self.cols
    }
    pub fn visible_rows(&self) -> usize {
        self.visible_rows
    }
    pub fn scrollback_len(&self) -> usize {
        self.rows.len().saturating_sub(self.visible_rows)
    }

    /// Total scrollback lines evicted off the top since creation (monotonic).
    pub fn lines_evicted(&self) -> u64 {
        self.lines_evicted
    }

    fn visible_start(&self) -> usize {
        self.rows.len().saturating_sub(self.visible_rows)
    }

    fn row_mut(&mut self, row: usize) -> &mut Row {
        let idx = self.visible_start() + row.min(self.visible_rows.saturating_sub(1));
        &mut self.rows[idx]
    }

    pub fn row(&self, row: usize) -> &Row {
        let idx = self.visible_start() + row.min(self.visible_rows.saturating_sub(1));
        &self.rows[idx]
    }

    /// Iterate visible rows top-to-bottom.
    pub fn visible_iter(&self) -> impl Iterator<Item = &Row> {
        let start = self.visible_start();
        let end = (start + self.visible_rows).min(self.rows.len());
        self.rows[start..end].iter()
    }

    /// Iterate all rows (scrollback first, then visible) top-to-bottom.
    /// Used by renderers that draw the scrollback region — pair with
    /// `RenderSnapshot::visible_rows` to know which trailing rows are
    /// "currently visible" vs scrollback.
    pub fn iter_all(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter()
    }

    // ─── Printing ──────────────────────────────────────────────────────────

    /// Print one grapheme base character at the cursor; advances the cursor
    /// by 1 (callers handle wide-char spacing separately).
    pub fn print(&mut self, c: char) {
        if self.auto_wrap && self.cursor_col >= self.cols {
            let cols = self.cols;
            if cols > 0 {
                self.row_mut(self.cursor_row).cells[cols - 1]
                    .flags
                    .set(CellFlags::WRAPLINE);
            }
            self.cursor_col = 0;
            self.linefeed();
        }
        let col = self.cursor_col.min(self.cols.saturating_sub(1));
        let (fg, bg, flags) = (self.current_fg, self.current_bg, self.current_flags);

        // Attach OSC 8 hyperlink (sticky) and OSC 133 prompt marker
        // (one-shot — taken here, not on subsequent prints).
        let mut extra: Option<Box<CellExtra>> = None;
        let next_prompt = self.next_prompt.take();
        if let Some(prompt) = next_prompt {
            extra.get_or_insert_with(Box::default).prompt = Some(prompt);
        }
        if let Some((_, url)) = &self.current_hyperlink {
            if !url.is_empty() {
                extra.get_or_insert_with(Box::default).hyperlink = Some(url.clone());
            }
        }

        let cell = &mut self.row_mut(self.cursor_row).cells[col];
        *cell = Cell {
            c,
            fg,
            bg,
            flags,
            extra,
        };
        self.cursor_col = col + 1;
        self.last_printed = Some(c);
    }

    /// Append a combining mark to the most recently printed cell.
    pub fn push_zerowidth(&mut self, c: char) {
        if self.cols == 0 {
            return;
        }
        let col = self.cursor_col.saturating_sub(1).min(self.cols - 1);
        let cell = &mut self.row_mut(self.cursor_row).cells[col];
        cell.push_zerowidth(c);
    }

    /// **REP** — repeat the last printed character.
    pub fn repeat_last(&mut self, n: usize) {
        if let Some(c) = self.last_printed {
            for _ in 0..n {
                self.print(c);
            }
        }
    }

    // ─── Cursor moves ──────────────────────────────────────────────────────

    pub fn cursor_up(&mut self, n: usize) {
        self.cursor_row = self.cursor_row.saturating_sub(n).max(self.scroll_top);
    }
    pub fn cursor_down(&mut self, n: usize) {
        self.cursor_row = (self.cursor_row + n).min(self.scroll_bottom);
    }
    pub fn cursor_forward(&mut self, n: usize) {
        self.cursor_col = (self.cursor_col + n).min(self.cols.saturating_sub(1));
    }
    pub fn cursor_back(&mut self, n: usize) {
        self.cursor_col = self.cursor_col.saturating_sub(n);
    }
    pub fn cursor_next_line(&mut self, n: usize) {
        self.cursor_down(n);
        self.cursor_col = 0;
    }
    pub fn cursor_prev_line(&mut self, n: usize) {
        self.cursor_up(n);
        self.cursor_col = 0;
    }
    pub fn cursor_column(&mut self, col_1based: usize) {
        self.cursor_col = col_1based.saturating_sub(1).min(self.cols.saturating_sub(1));
    }
    pub fn cursor_vertical(&mut self, row_1based: usize) {
        self.cursor_row = row_1based
            .saturating_sub(1)
            .min(self.visible_rows.saturating_sub(1));
    }
    /// CUP / HVP — honours DECOM origin mode.
    pub fn cursor_position(&mut self, row_1based: usize, col_1based: usize) {
        let row = row_1based.saturating_sub(1);
        let col = col_1based.saturating_sub(1);
        let (row_max, row_offset) = if self.origin_mode {
            (
                self.scroll_bottom.saturating_sub(self.scroll_top),
                self.scroll_top,
            )
        } else {
            (self.visible_rows.saturating_sub(1), 0)
        };
        self.cursor_row = (row + row_offset).min(self.scroll_bottom).min(row_max + row_offset);
        self.cursor_col = col.min(self.cols.saturating_sub(1));
    }
    pub fn next_tab(&mut self, n: usize) {
        for _ in 0..n {
            let next = ((self.cursor_col / 8) + 1) * 8;
            self.cursor_col = next.min(self.cols.saturating_sub(1));
        }
    }
    pub fn prev_tab(&mut self, n: usize) {
        for _ in 0..n {
            let prev = (self.cursor_col.saturating_sub(1) / 8) * 8;
            self.cursor_col = prev;
        }
    }

    // ─── Erase / insert / delete on the current row ───────────────────────

    /// **ECH** — erase N cells at the cursor without moving it.
    pub fn erase_chars(&mut self, n: usize) {
        let start = self.cursor_col;
        let end = (start + n).min(self.cols);
        self.row_mut(self.cursor_row).clear_range(start..end);
    }

    /// **ICH** — insert N blank cells at the cursor.
    pub fn insert_chars(&mut self, n: usize) {
        let cols = self.cols;
        let col = self.cursor_col.min(cols);
        let row = self.row_mut(self.cursor_row);
        let count = n.min(cols - col);
        if count == 0 {
            return;
        }
        row.cells[col..].rotate_right(count);
        for cell in &mut row.cells[col..col + count] {
            cell.reset();
        }
    }

    /// **DCH** — delete N cells at the cursor.
    pub fn delete_chars(&mut self, n: usize) {
        let cols = self.cols;
        let col = self.cursor_col.min(cols);
        let row = self.row_mut(self.cursor_row);
        let count = n.min(cols - col);
        if count == 0 {
            return;
        }
        row.cells[col..].rotate_left(count);
        for cell in &mut row.cells[cols - count..] {
            cell.reset();
        }
    }

    /// **IL** — insert N blank lines at the cursor row (within scroll region).
    pub fn insert_lines(&mut self, n: usize) {
        if self.cursor_row < self.scroll_top || self.cursor_row > self.scroll_bottom {
            return;
        }
        let cols = self.cols;
        let n = n.min(self.scroll_bottom - self.cursor_row + 1);
        for _ in 0..n {
            let remove_idx = self.visible_start() + self.scroll_bottom;
            if remove_idx < self.rows.len() {
                self.rows.remove(remove_idx);
            }
            let insert_idx = self.visible_start() + self.cursor_row;
            self.rows.insert(insert_idx, Row::new(cols));
        }
    }

    /// **DL** — delete N lines at the cursor row (within scroll region).
    pub fn delete_lines(&mut self, n: usize) {
        if self.cursor_row < self.scroll_top || self.cursor_row > self.scroll_bottom {
            return;
        }
        let cols = self.cols;
        let n = n.min(self.scroll_bottom - self.cursor_row + 1);
        for _ in 0..n {
            let remove_idx = self.visible_start() + self.cursor_row;
            if remove_idx < self.rows.len() {
                self.rows.remove(remove_idx);
            }
            let insert_idx = self.visible_start() + self.scroll_bottom;
            self.rows.insert(insert_idx, Row::new(cols));
        }
    }

    // ─── LF / CR / scroll ──────────────────────────────────────────────────

    pub fn linefeed(&mut self) {
        if self.cursor_row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor_row + 1 < self.visible_rows {
            self.cursor_row += 1;
        }
    }

    pub fn reverse_index(&mut self) {
        if self.cursor_row == self.scroll_top {
            self.scroll_down(1);
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    /// Scroll the active region up by `n` rows. Rows leaving the top spill
    /// into scrollback when `scroll_top == 0`.
    pub fn scroll_up(&mut self, n: usize) {
        let cols = self.cols;
        for _ in 0..n {
            if self.scroll_top == 0 {
                if self.scrollback_len() >= self.max_scrollback {
                    self.rows.remove(0);
                    self.lines_evicted += 1;
                }
                let insert_idx = self.visible_start() + self.scroll_bottom + 1;
                let insert_idx = insert_idx.min(self.rows.len());
                self.rows.insert(insert_idx, Row::new(cols));
            } else {
                let remove_idx = self.visible_start() + self.scroll_top;
                self.rows.remove(remove_idx);
                let insert_idx = self.visible_start() + self.scroll_bottom;
                self.rows.insert(insert_idx, Row::new(cols));
            }
        }
    }

    pub fn scroll_down(&mut self, n: usize) {
        let cols = self.cols;
        for _ in 0..n {
            let remove_idx = self.visible_start() + self.scroll_bottom;
            if remove_idx < self.rows.len() {
                self.rows.remove(remove_idx);
            }
            let insert_idx = self.visible_start() + self.scroll_top;
            self.rows.insert(insert_idx, Row::new(cols));
        }
    }

    pub fn set_scroll_region(&mut self, top_1based: u16, bottom_1based: u16) {
        let top = top_1based.saturating_sub(1) as usize;
        let bottom = if bottom_1based == u16::MAX {
            self.visible_rows.saturating_sub(1)
        } else {
            (bottom_1based.saturating_sub(1) as usize).min(self.visible_rows.saturating_sub(1))
        };
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        }
        self.cursor_row = if self.origin_mode { top } else { 0 };
        self.cursor_col = 0;
    }

    // ─── Erase larger regions ──────────────────────────────────────────────

    pub fn erase_display(&mut self, mode: super::parser::EraseMode) {
        use super::parser::EraseMode;
        let cols = self.cols;
        match mode {
            EraseMode::ToEnd => {
                self.erase_line(EraseMode::ToEnd);
                for r in (self.cursor_row + 1)..self.visible_rows {
                    self.row_mut(r).clear_range(0..cols);
                }
            }
            EraseMode::ToStart => {
                for r in 0..self.cursor_row {
                    self.row_mut(r).clear_range(0..cols);
                }
                self.erase_line(EraseMode::ToStart);
            }
            EraseMode::All => {
                for r in 0..self.visible_rows {
                    self.row_mut(r).clear_range(0..cols);
                }
            }
            EraseMode::Scrollback => {
                let start = self.visible_start();
                self.rows.drain(0..start);
            }
        }
    }

    pub fn erase_line(&mut self, mode: super::parser::EraseMode) {
        use super::parser::EraseMode;
        let cols = self.cols;
        let col = self.cursor_col;
        let row = self.row_mut(self.cursor_row);
        match mode {
            EraseMode::All => row.clear_range(0..cols),
            EraseMode::ToEnd => row.clear_range(col..cols),
            EraseMode::ToStart => row.clear_range(0..(col + 1).min(cols)),
            EraseMode::Scrollback => {}
        }
    }

    // ─── Cursor save/restore ───────────────────────────────────────────────

    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some(SavedCursor {
            row: self.cursor_row,
            col: self.cursor_col,
            fg: self.current_fg,
            bg: self.current_bg,
            flags: self.current_flags,
        });
    }

    pub fn restore_cursor(&mut self) {
        if let Some(s) = self.saved_cursor {
            self.cursor_row = s.row.min(self.visible_rows.saturating_sub(1));
            self.cursor_col = s.col.min(self.cols.saturating_sub(1));
            self.current_fg = s.fg;
            self.current_bg = s.bg;
            self.current_flags = s.flags;
        }
    }

    pub fn save_cursor_sco(&mut self) {
        self.saved_cursor_sco = Some((self.cursor_row, self.cursor_col));
    }

    pub fn restore_cursor_sco(&mut self) {
        if let Some((r, c)) = self.saved_cursor_sco {
            self.cursor_row = r.min(self.visible_rows.saturating_sub(1));
            self.cursor_col = c.min(self.cols.saturating_sub(1));
        }
    }

    // ─── Alt screen ────────────────────────────────────────────────────────

    pub fn enter_alt_screen(&mut self) {
        if self.alt_rows.is_some() {
            return;
        }
        let cols = self.cols;
        let rows = self.visible_rows;
        let alt: Vec<Row> = (0..rows).map(|_| Row::new(cols)).collect();
        self.alt_rows = Some(std::mem::replace(&mut self.rows, alt));
        self.alt_cursor = Some((self.cursor_row, self.cursor_col));
        // Snapshot the primary SGR template so we can restore it on
        // exit; reset attrs to defaults for the alt screen so stale
        // BOLD / UNDERLINE / custom fg / bg from the primary don't
        // bleed into the freshly-entered alt frame.
        self.alt_sgr = Some((self.current_fg, self.current_bg, self.current_flags));
        self.current_fg = TermColor::Default;
        self.current_bg = TermColor::Default;
        self.current_flags = CellFlags::empty();
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    pub fn exit_alt_screen(&mut self) {
        if let Some(rows) = self.alt_rows.take() {
            self.rows = rows;
        }
        if let Some((r, c)) = self.alt_cursor.take() {
            self.cursor_row = r;
            self.cursor_col = c;
        }
        // Restore the primary screen's SGR template — symmetric with
        // the snapshot in `enter_alt_screen`.
        if let Some((fg, bg, flags)) = self.alt_sgr.take() {
            self.current_fg = fg;
            self.current_bg = bg;
            self.current_flags = flags;
        }
    }

    // ─── Resize / reset ────────────────────────────────────────────────────

    pub fn resize(&mut self, cols: usize, rows: usize) {
        // Reflow soft-wrapped content when the column count changes.
        // Modelled on Warp's `Index::rebuild` (see warp_terminal/.../index.rs):
        // walk cells through the WRAPLINE chain to produce flat logical
        // lines, then re-emit rows at the new width. Hard line breaks
        // (no WRAPLINE on the trailing cell) survive intact.
        //
        // We track the cursor's absolute row across both reflow and the
        // outer pad/truncate step, then project back to a visible-relative
        // row at the very end. Doing the visible-relative conversion
        // before pad/truncate would race with the moving visible_start.
        let cursor_abs_before = self.visible_start()
            + self.cursor_row.min(self.visible_rows.saturating_sub(1));
        let cursor_abs = if cols != self.cols && cols > 0 {
            self.reflow_columns(cols).unwrap_or(cursor_abs_before)
        } else {
            cursor_abs_before
        };

        for row in &mut self.rows {
            row.resize(cols);
        }
        // Top-anchored resize: the visible region always pins to the top
        // of the row buffer. Content never scrolls in response to window
        // size changes.
        //
        // When visible_rows grows, we let it absorb existing scrollback
        // (so older content re-enters the viewport) instead of preserving
        // the old scrollback row count verbatim. This matches the user's
        // mental model of "content does not move on resize" — a taller
        // window simply sees more rows, top first.
        //
        // When visible_rows shrinks (or stays the same), scrollback is
        // preserved; trailing blank rows beyond the new visible region
        // are truncated.
        let prev_scrollback = self.rows.len().saturating_sub(self.visible_rows);
        let visible_increment = rows.saturating_sub(self.visible_rows);
        let scrollback_to_keep = prev_scrollback.saturating_sub(visible_increment);
        let target = scrollback_to_keep + rows;
        if self.rows.len() < target {
            while self.rows.len() < target {
                self.rows.push(Row::new(cols));
            }
        } else if self.rows.len() > target {
            self.rows.truncate(target);
        }
        self.cols = cols;
        self.visible_rows = rows;
        self.scroll_bottom = rows.saturating_sub(1);
        // Project the cursor's absolute row back to the new visible region.
        let visible_start = self.rows.len().saturating_sub(self.visible_rows);
        self.cursor_row = cursor_abs
            .saturating_sub(visible_start)
            .min(rows.saturating_sub(1));
        if self.cursor_col >= cols {
            self.cursor_col = cols.saturating_sub(1);
        }
    }

    /// Rebuilds rows at `new_cols`, preserving soft-wrapped logical lines.
    /// Returns the cursor's new absolute row in the rebuilt buffer (for the
    /// outer `resize` to convert to visible-relative once pad/truncate
    /// settles `visible_start`).
    fn reflow_columns(&mut self, new_cols: usize) -> Option<usize> {
        let old_cols = self.cols;
        if old_cols == 0 {
            return None;
        }

        let cursor_abs_row =
            self.visible_start() + self.cursor_row.min(self.visible_rows.saturating_sub(1));
        let (cur_line, cur_offset) =
            locate_cursor_logical(&self.rows, old_cols, cursor_abs_row, self.cursor_col);

        let logical = collect_logical_lines(&self.rows, old_cols);
        let new_rows = rewrap(&logical, new_cols);
        let (new_abs_row, new_col) =
            place_cursor_logical(&logical, cur_line, cur_offset, new_cols);

        self.rows = new_rows;
        self.cursor_col = new_col;
        Some(new_abs_row)
    }

    pub fn reset(&mut self) {
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.cursor_visible = true;
        self.cursor_style = CursorStyle::BlockSteady;
        self.current_fg = TermColor::Default;
        self.current_bg = TermColor::Default;
        self.current_flags = CellFlags::empty();
        self.current_hyperlink = None;
        self.next_prompt = None;
        self.scroll_top = 0;
        self.scroll_bottom = self.visible_rows.saturating_sub(1);
        self.origin_mode = false;
        self.auto_wrap = true;
        self.bracketed_paste = false;
        self.focus_reporting = false;
        self.sync_output = false;
        self.mouse = MouseProtocol::default();
        self.cursor_keys_app = false;
        self.keypad_app = false;
        self.last_printed = None;
        for r in 0..self.visible_rows {
            let cols = self.cols;
            self.row_mut(r).clear_range(0..cols);
        }
    }
}

// ──── Reflow helpers ──────────────────────────────────────────────────────
//
// Logical line = concatenated cells from a chain of rows joined by
// CellFlags::WRAPLINE on the trailing cell. Rebuilding rows at a new
// column count is then a plain `chunks(new_cols)` over the trimmed
// content, with WRAPLINE set on the last cell of every chunk except
// the final one.

struct LogicalLine {
    cells: Vec<Cell>,
}

fn row_wraps(row: &Row, old_cols: usize) -> bool {
    if old_cols == 0 {
        return false;
    }
    row.cells
        .get(old_cols - 1)
        .map(|c| c.flags.wrap_line())
        .unwrap_or(false)
}

fn trim_trailing_blanks(cells: &[Cell]) -> usize {
    let blank = Cell::space();
    let mut len = cells.len();
    while len > 0 && cells[len - 1] == blank {
        len -= 1;
    }
    len
}

fn collect_logical_lines(rows: &[Row], old_cols: usize) -> Vec<LogicalLine> {
    let mut out = Vec::with_capacity(rows.len());
    let mut current: Vec<Cell> = Vec::new();
    for row in rows {
        current.extend(row.cells.iter().cloned());
        if !row_wraps(row, old_cols) {
            out.push(LogicalLine {
                cells: std::mem::take(&mut current),
            });
        }
    }
    // Final dangling chain (last row had WRAPLINE) — defensive; should not
    // normally happen since the bottom row of the buffer is the cursor's
    // current line and hasn't wrapped yet.
    if !current.is_empty() {
        out.push(LogicalLine { cells: current });
    }
    // Drop trailing logical lines that are entirely blank. They represent
    // the "below the cursor" empty area in the source buffer; the outer
    // `Grid::resize` re-creates those blank rows by padding to fit the
    // visible region, so emitting them here would double up and push real
    // content into scrollback under our bottom-anchored visible window.
    while out
        .last()
        .map(|l| trim_trailing_blanks(&l.cells) == 0)
        .unwrap_or(false)
    {
        out.pop();
    }
    out
}

fn rewrap(logical: &[LogicalLine], new_cols: usize) -> Vec<Row> {
    debug_assert!(new_cols > 0);
    let mut out = Vec::with_capacity(logical.len());
    for line in logical {
        let trimmed = trim_trailing_blanks(&line.cells);
        if trimmed == 0 {
            out.push(Row::new(new_cols));
            continue;
        }
        let mut cells: Vec<Cell> = line.cells[..trimmed].to_vec();
        // WRAPLINE was set on the OLD column boundary inside each row;
        // clear it everywhere so we can re-set it at the new boundaries.
        for cell in &mut cells {
            cell.flags.clear(CellFlags::WRAPLINE);
        }
        let mut start = 0;
        while start < cells.len() {
            let end = (start + new_cols).min(cells.len());
            let mut row = Row::new(new_cols);
            for (i, cell) in cells[start..end].iter().enumerate() {
                row.cells[i] = cell.clone();
            }
            if end < cells.len() {
                row.cells[new_cols - 1].flags.set(CellFlags::WRAPLINE);
            }
            out.push(row);
            start = end;
        }
    }
    out
}

fn locate_cursor_logical(
    rows: &[Row],
    old_cols: usize,
    abs_row: usize,
    col: usize,
) -> (usize, usize) {
    let mut logical_idx = 0;
    let mut offset_in_line = 0;
    for (idx, row) in rows.iter().enumerate() {
        let wraps = row_wraps(row, old_cols);
        if idx == abs_row {
            return (logical_idx, offset_in_line + col.min(old_cols));
        }
        offset_in_line += old_cols;
        if !wraps {
            logical_idx += 1;
            offset_in_line = 0;
        }
    }
    (logical_idx, offset_in_line)
}

fn place_cursor_logical(
    logical: &[LogicalLine],
    line_idx: usize,
    offset_in_line: usize,
    new_cols: usize,
) -> (usize, usize) {
    let mut abs_row = 0;
    for (i, line) in logical.iter().enumerate() {
        let trimmed = trim_trailing_blanks(&line.cells);
        let rows_for_line = if trimmed == 0 {
            1
        } else {
            (trimmed + new_cols - 1) / new_cols
        };
        if i == line_idx {
            let capped = offset_in_line.min(trimmed);
            let row_off = (capped / new_cols).min(rows_for_line.saturating_sub(1));
            let col = (capped % new_cols).min(new_cols.saturating_sub(1));
            return (abs_row + row_off, col);
        }
        abs_row += rows_for_line;
    }
    (abs_row, 0)
}
