//! Fixed-cell terminal grid primitives.
//!
//! Bootstrap commit ships `Cell` + `CellExtra` + `PromptMarker` only.
//! `Row` and `Grid` (with cursor, scroll region, alt screen) land in the
//! grid commit. The split keeps each commit atomic and compiling.

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
