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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode {
    #[default]
    None,
    X10,
    ButtonEvent,
    AnyEvent,
    Sgr,
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

    /// Modes.
    pub origin_mode: bool,
    pub auto_wrap: bool,
    pub bracketed_paste: bool,
    pub focus_reporting: bool,
    pub sync_output: bool,
    pub mouse_mode: MouseMode,
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
            origin_mode: false,
            auto_wrap: true,
            bracketed_paste: false,
            focus_reporting: false,
            sync_output: false,
            mouse_mode: MouseMode::None,
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

    // ─── Printing ──────────────────────────────────────────────────────────

    /// Print one grapheme base character at the cursor; advances the cursor
    /// by 1 (callers handle wide-char spacing separately).
    pub fn print(&mut self, c: char) {
        if self.auto_wrap && self.cursor_col >= self.cols {
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
    }

    // ─── Resize / reset ────────────────────────────────────────────────────

    pub fn resize(&mut self, cols: usize, rows: usize) {
        for row in &mut self.rows {
            row.resize(cols);
        }
        while self.rows.len() < self.visible_start() + rows {
            self.rows.push(Row::new(cols));
        }
        self.cols = cols;
        self.visible_rows = rows;
        self.scroll_bottom = rows.saturating_sub(1);
        if self.cursor_row >= rows {
            self.cursor_row = rows.saturating_sub(1);
        }
        if self.cursor_col >= cols {
            self.cursor_col = cols.saturating_sub(1);
        }
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
        self.mouse_mode = MouseMode::None;
        self.cursor_keys_app = false;
        self.keypad_app = false;
        self.last_printed = None;
        for r in 0..self.visible_rows {
            let cols = self.cols;
            self.row_mut(r).clear_range(0..cols);
        }
    }
}
