//! GPU-based UI for anyclaude, built on `term_gpu` / `term_core` /
//! `term_layout` / `term_clipboard` / `term_ui` / `uikit`.
//!
//! This is the live UI: `main.rs` routes straight to [`run`]. It replaced the
//! legacy ratatui pipeline in the Phase 5 cutover (the old `--gpu` flag and the
//! `ui::run` entry are gone). Chrome + popups render as `term_ui` retained
//! trees in the overlay; the terminal grid is a direct `populate_panel`
//! full-emit each frame (R5). The coordinator lives in [`app`].

pub mod app;
mod backends;
mod bootstrap;
mod chrome;
mod diagnostic;
mod overlay;
pub mod pty;
mod session;
mod text;
mod timers;

pub use bootstrap::run;
