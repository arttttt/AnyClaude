pub mod app;
pub mod events;
pub mod footer;
pub mod header;
pub mod history;
pub mod input;
pub mod layout;
pub mod mvi;
pub mod popup;
pub mod pty_state;
pub mod render;
pub mod runtime;
pub mod summarization;
pub mod terminal;
pub mod terminal_guard;
pub mod theme;

pub use runtime::run;
