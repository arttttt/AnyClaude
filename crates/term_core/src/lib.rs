//! Minimal VT/ANSI parser and fixed-cell terminal grid.
//!
//! Hand-rolled Paul Williams state machine (no `vte` dep) and an
//! alacritty-style `Cell` grid. Variable-width rendering happens in
//! `term_gpu` — `term_core` is logically monospace for VT correctness.
//!
//! Boundaries:
//! - VT support: P0 + P1 sequences from
//!   `docs/analysis/warp-vt-parser-research.md` §3. See spec §4.2.
//! - Out of scope: tmux control mode, image protocols, kitty keyboard,
//!   sixel, DEC charset G2/G3. See spec §4.3.
//!
//! Bootstrap commit: only the primitive types (colour, cell flags,
//! `Cell`) ship. Parser, grid operations, and the emulator land in
//! subsequent commits.

pub mod attrs;
pub mod color;
pub mod emulator;
pub mod grid;
pub mod parser;

pub use attrs::CellFlags;
pub use color::{AnsiPalette, TermColor};
pub use emulator::{CursorState, RenderSnapshot, TerminalEmulator, VtEmulator};
pub use grid::{
    Cell, CellExtra, CursorStyle, Grid, MouseEncoding, MouseProtocol, MouseTracking, PromptMarker,
    Row,
};
pub use parser::{Action, EraseMode, Parser, PromptKind, SgrAction, TabClear};

/// Create a terminal emulator with the given visible grid size and
/// scrollback line cap.
pub fn create_emulator(cols: usize, rows: usize, max_scrollback: usize) -> Box<dyn TerminalEmulator> {
    Box::new(emulator::VtEmulator::new(cols, rows, max_scrollback))
}
