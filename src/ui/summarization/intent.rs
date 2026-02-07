//! Intents for the summarization dialog.

use crate::ui::mvi::Intent;

/// Intents that can be dispatched to the summarization dialog.
#[derive(Debug, Clone)]
pub enum SummarizeIntent {
    /// Start the summarization process.
    Start,

    /// Animation tick (for spinner updates).
    AnimationTick,

    /// Summarization completed successfully.
    Success,

    /// An error occurred during summarization.
    Error {
        /// Error message.
        message: String,
    },

    /// Toggle button selection in Failed state.
    ToggleButton,

    /// Hide the dialog (after cancel).
    Hide,
}

impl Intent for SummarizeIntent {}
