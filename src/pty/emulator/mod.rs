mod alacritty_impl;

/// Terminal color representation, independent of any specific emulator library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// A single terminal cell with owned data.
#[derive(Debug, Clone)]
pub struct TermCell {
    pub symbol: String,
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub has_contents: bool,
    pub is_wide_continuation: bool,
}

/// Cursor position and visibility.
#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
}

/// Backend-agnostic terminal emulator interface.
///
/// Implementations wrap a concrete terminal emulation library (e.g.
/// `alacritty_terminal`) and expose a uniform API consumed by the rest of the
/// codebase.  Only the implementation file should depend on the underlying
/// library crate.
pub trait TerminalEmulator: Send {
    /// Feed raw bytes from the PTY into the emulator.
    fn process(&mut self, bytes: &[u8]);

    /// Resize the virtual terminal.
    fn set_size(&mut self, rows: u16, cols: u16);

    /// Read a single cell at `(row, col)`.  Returns `None` if out of bounds.
    fn cell(&self, row: u16, col: u16) -> Option<TermCell>;

    /// Current scrollback offset (0 = live view).
    fn scrollback(&self) -> usize;

    /// Set the scrollback offset.
    fn set_scrollback(&mut self, offset: usize);

    /// Current cursor state.
    fn cursor(&self) -> CursorState;
}

/// Create a terminal emulator backed by the default implementation.
///
/// This is the single point where the concrete backend is chosen.  To switch
/// to a different library, change only this function and the implementation
/// module.
pub fn create(rows: u16, cols: u16, scrollback_len: usize) -> Box<dyn TerminalEmulator> {
    Box::new(alacritty_impl::AlacrittyEmulator::new(rows, cols, scrollback_len))
}
