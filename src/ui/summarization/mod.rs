//! Summarization dialog feature module.
//!
//! This module implements the UI for the session summarization dialog
//! that appears when switching backends in summarize mode.
//!
//! # Architecture
//!
//! Uses MVI (Model-View-Intent) pattern:
//! - `state.rs` - Dialog state enum
//! - `intent.rs` - User/system actions
//! - `reducer.rs` - State transitions
//! - `dialog.rs` - Rendering

mod dialog;
mod intent;
mod reducer;
mod state;

pub use dialog::render_summarize_dialog;
pub use intent::SummarizeIntent;
pub use reducer::SummarizeReducer;
pub use state::{SummarizeDialogState, MAX_AUTO_RETRIES};
