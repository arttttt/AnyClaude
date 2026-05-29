//! `uikit` — a reusable, domain-agnostic widget kit for AnyClaude, composed
//! over [`term_ui`].
//!
//! It sits one layer ABOVE the `term_ui` engine (arena / Element / layout /
//! paint) and one layer BELOW the anyclaude binary: it assembles `term_ui`
//! `Stack`/`Text`/`Spacer`/`Block` elements into reusable widgets (chrome
//! bars, …) but knows NOTHING of anyclaude's domain — no `BackendState`, no
//! session id, no PTY, no "backend:/Reqs:" presentation strings. Those live in
//! the binary's presenter, which feeds this kit already-formatted [`Segment`]s.
//!
//! Design: `docs/design/term-ui-design.md` (Phase C — porting the real chrome
//! to declarative `term_ui` views).
//!
//! Scope (KISS/YAGNI): Phase C is the chrome bars only. New widgets land here
//! as their consumers appear, never before.

pub mod chrome;
pub mod popup;

pub use chrome::{footer_bar, header_bar, Segment};
pub use popup::{fixed_row_window, popup_list};
