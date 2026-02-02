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
    Success {
        /// Preview of the generated summary.
        summary_preview: String,
    },

    /// An error occurred during summarization.
    Error {
        /// Error message.
        message: String,
    },

    /// User clicked the Retry button.
    RetryClicked,

    /// User clicked the Cancel button.
    CancelClicked,

    /// Hide the dialog (after success or cancel).
    Hide,
}

impl Intent for SummarizeIntent {}
