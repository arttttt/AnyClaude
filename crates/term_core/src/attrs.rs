//! Per-cell text attribute flags.

/// Bit-packed cell flags. `u16` because we already use 10 bits; future
/// flags (e.g. extended underline styles) won't need to widen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CellFlags(u16);

impl CellFlags {
    pub const BOLD: u16 = 1 << 0;
    pub const FAINT: u16 = 1 << 1;
    pub const ITALIC: u16 = 1 << 2;
    pub const UNDERLINE: u16 = 1 << 3;
    pub const DOUBLE_UNDERLINE: u16 = 1 << 4;
    pub const INVERSE: u16 = 1 << 5;
    pub const STRIKE: u16 = 1 << 6;
    pub const BLINK_SLOW: u16 = 1 << 7;
    pub const BLINK_FAST: u16 = 1 << 8;
    pub const HIDDEN: u16 = 1 << 9;
    /// Left half of a wide character (e.g. CJK, emoji). The right half is
    /// the spacer cell that follows.
    pub const WIDE_CHAR: u16 = 1 << 10;
    /// Right half of a wide character — placeholder, has no glyph of its own.
    pub const WIDE_CHAR_SPACER: u16 = 1 << 11;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn bits(self) -> u16 {
        self.0
    }

    pub fn contains(self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    pub fn set(&mut self, flag: u16) {
        self.0 |= flag;
    }

    pub fn clear(&mut self, flag: u16) {
        self.0 &= !flag;
    }

    pub fn reset(&mut self) {
        self.0 = 0;
    }

    pub fn bold(self) -> bool {
        self.contains(Self::BOLD)
    }
    pub fn faint(self) -> bool {
        self.contains(Self::FAINT)
    }
    pub fn italic(self) -> bool {
        self.contains(Self::ITALIC)
    }
    pub fn underline(self) -> bool {
        self.contains(Self::UNDERLINE)
    }
    pub fn double_underline(self) -> bool {
        self.contains(Self::DOUBLE_UNDERLINE)
    }
    pub fn inverse(self) -> bool {
        self.contains(Self::INVERSE)
    }
    pub fn strike(self) -> bool {
        self.contains(Self::STRIKE)
    }
    pub fn blink_slow(self) -> bool {
        self.contains(Self::BLINK_SLOW)
    }
    pub fn blink_fast(self) -> bool {
        self.contains(Self::BLINK_FAST)
    }
    pub fn hidden(self) -> bool {
        self.contains(Self::HIDDEN)
    }
    pub fn wide_char(self) -> bool {
        self.contains(Self::WIDE_CHAR)
    }
    pub fn wide_char_spacer(self) -> bool {
        self.contains(Self::WIDE_CHAR_SPACER)
    }
}
