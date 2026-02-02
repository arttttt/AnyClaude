//! Reducer for the summarization dialog.

use crate::ui::mvi::Reducer;

use super::intent::SummarizeIntent;
use super::state::{SummarizeDialogState, MAX_AUTO_RETRIES};

/// Reducer for summarization dialog state transitions.
pub struct SummarizeReducer;

impl Reducer for SummarizeReducer {
    type State = SummarizeDialogState;
    type Intent = SummarizeIntent;

    fn reduce(state: Self::State, intent: Self::Intent) -> Self::State {
        match intent {
            SummarizeIntent::Start => SummarizeDialogState::Summarizing { animation_tick: 0 },

            SummarizeIntent::AnimationTick => match state {
                SummarizeDialogState::Summarizing { animation_tick } => {
                    SummarizeDialogState::Summarizing {
                        animation_tick: animation_tick.wrapping_add(1),
                    }
                }
                SummarizeDialogState::Retrying {
                    attempt,
                    error,
                    animation_tick,
                } => SummarizeDialogState::Retrying {
                    attempt,
                    error,
                    animation_tick: animation_tick.wrapping_add(1),
                },
                other => other,
            },

            SummarizeIntent::Success { summary_preview } => {
                SummarizeDialogState::Success { summary_preview }
            }

            SummarizeIntent::Error { message } => {
                // Determine current attempt number
                let current_attempt = match &state {
                    SummarizeDialogState::Summarizing { .. } => 1,
                    SummarizeDialogState::Retrying { attempt, .. } => attempt + 1,
                    _ => 1,
                };

                if current_attempt >= MAX_AUTO_RETRIES {
                    // Max retries reached, show user choice
                    SummarizeDialogState::Failed { error: message }
                } else {
                    // Auto-retry
                    SummarizeDialogState::Retrying {
                        attempt: current_attempt,
                        error: message,
                        animation_tick: 0,
                    }
                }
            }

            SummarizeIntent::RetryClicked => {
                // User chose to retry, reset to summarizing state
                SummarizeDialogState::Summarizing { animation_tick: 0 }
            }

            SummarizeIntent::CancelClicked | SummarizeIntent::Hide => {
                SummarizeDialogState::Hidden
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_transitions_to_summarizing() {
        let state = SummarizeDialogState::Hidden;
        let new_state = SummarizeReducer::reduce(state, SummarizeIntent::Start);
        assert!(matches!(
            new_state,
            SummarizeDialogState::Summarizing { animation_tick: 0 }
        ));
    }

    #[test]
    fn animation_tick_increments() {
        let state = SummarizeDialogState::Summarizing { animation_tick: 5 };
        let new_state = SummarizeReducer::reduce(state, SummarizeIntent::AnimationTick);
        assert!(matches!(
            new_state,
            SummarizeDialogState::Summarizing { animation_tick: 6 }
        ));
    }

    #[test]
    fn first_error_transitions_to_retrying() {
        let state = SummarizeDialogState::Summarizing { animation_tick: 0 };
        let new_state = SummarizeReducer::reduce(
            state,
            SummarizeIntent::Error {
                message: "timeout".into(),
            },
        );

        match new_state {
            SummarizeDialogState::Retrying { attempt, error, .. } => {
                assert_eq!(attempt, 1);
                assert_eq!(error, "timeout");
            }
            _ => panic!("Expected Retrying state"),
        }
    }

    #[test]
    fn max_retries_transitions_to_failed() {
        // Simulate being on attempt 2 (so next error is attempt 3 = MAX)
        let state = SummarizeDialogState::Retrying {
            attempt: 2,
            error: "previous".into(),
            animation_tick: 0,
        };

        let new_state = SummarizeReducer::reduce(
            state,
            SummarizeIntent::Error {
                message: "final error".into(),
            },
        );

        match new_state {
            SummarizeDialogState::Failed { error } => {
                assert_eq!(error, "final error");
            }
            _ => panic!("Expected Failed state"),
        }
    }

    #[test]
    fn retry_clicked_restarts_summarizing() {
        let state = SummarizeDialogState::Failed {
            error: "error".into(),
        };
        let new_state = SummarizeReducer::reduce(state, SummarizeIntent::RetryClicked);
        assert!(matches!(
            new_state,
            SummarizeDialogState::Summarizing { animation_tick: 0 }
        ));
    }

    #[test]
    fn cancel_hides_dialog() {
        let state = SummarizeDialogState::Failed {
            error: "error".into(),
        };
        let new_state = SummarizeReducer::reduce(state, SummarizeIntent::CancelClicked);
        assert_eq!(new_state, SummarizeDialogState::Hidden);
    }

    #[test]
    fn success_stores_preview() {
        let state = SummarizeDialogState::Summarizing { animation_tick: 0 };
        let new_state = SummarizeReducer::reduce(
            state,
            SummarizeIntent::Success {
                summary_preview: "Summary text".into(),
            },
        );

        match new_state {
            SummarizeDialogState::Success { summary_preview } => {
                assert_eq!(summary_preview, "Summary text");
            }
            _ => panic!("Expected Success state"),
        }
    }
}
