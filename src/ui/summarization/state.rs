//! State for the summarization dialog.

use crate::ui::mvi::UiState;

/// Maximum number of automatic retries before showing user choice.
pub const MAX_AUTO_RETRIES: u8 = 3;

/// State of the summarization dialog.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SummarizeDialogState {
    /// Dialog is not visible.
    #[default]
    Hidden,

    /// Summarization is in progress.
    Summarizing {
        /// Animation tick for spinner.
        animation_tick: u8,
    },

    /// Retrying after an error.
    Retrying {
        /// Current retry attempt (1-based).
        attempt: u8,
        /// Error message from the last attempt.
        error: String,
        /// Animation tick for spinner.
        animation_tick: u8,
    },

    /// All automatic retries failed, waiting for user decision.
    Failed {
        /// Error message from the last attempt.
        error: String,
    },
}

impl UiState for SummarizeDialogState {}

impl SummarizeDialogState {
    /// Check if the dialog should be visible.
    pub fn is_visible(&self) -> bool {
        !matches!(self, Self::Hidden)
    }

    /// Check if user interaction is needed.
    pub fn needs_user_input(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    /// Check if animation should be running.
    pub fn is_animating(&self) -> bool {
        matches!(
            self,
            Self::Summarizing { .. } | Self::Retrying { .. }
        )
    }

    /// Get the current error message, if any.
    pub fn error_message(&self) -> Option<&str> {
        match self {
            Self::Retrying { error, .. } | Self::Failed { error } => Some(error),
            _ => None,
        }
    }

    /// Get the current retry attempt, if retrying.
    pub fn retry_attempt(&self) -> Option<u8> {
        match self {
            Self::Retrying { attempt, .. } => Some(*attempt),
            _ => None,
        }
    }

    /// Check if auto-retry should be triggered.
    pub fn should_auto_retry(&self) -> bool {
        matches!(self, Self::Retrying { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_is_default() {
        assert_eq!(SummarizeDialogState::default(), SummarizeDialogState::Hidden);
    }

    #[test]
    fn visibility_check() {
        assert!(!SummarizeDialogState::Hidden.is_visible());
        assert!(SummarizeDialogState::Summarizing { animation_tick: 0 }.is_visible());
        assert!(SummarizeDialogState::Failed {
            error: "test".into()
        }
        .is_visible());
    }

    #[test]
    fn needs_user_input_only_when_failed() {
        assert!(!SummarizeDialogState::Hidden.needs_user_input());
        assert!(!SummarizeDialogState::Summarizing { animation_tick: 0 }.needs_user_input());
        assert!(SummarizeDialogState::Failed {
            error: "test".into()
        }
        .needs_user_input());
    }
}
