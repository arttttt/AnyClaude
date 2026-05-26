//! GPU-based UI for anyclaude, built on `term_gpu` / `term_core` /
//! `term_layout` / `term_clipboard`.
//!
//! This module replaces the legacy ratatui rendering pipeline. It is
//! currently developed alongside the legacy `ui::run` entry — the
//! `--gpu` flag in `main.rs` routes here. Once the full feature
//! surface is ported (chrome, popups), the cutover commit makes this
//! the default and removes the flag.

pub mod app;
pub mod pty;

pub use app::run;
