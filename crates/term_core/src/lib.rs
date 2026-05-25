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
pub mod grid;

pub use attrs::CellFlags;
pub use color::{AnsiPalette, TermColor};
pub use grid::{Cell, CellExtra, PromptMarker};
