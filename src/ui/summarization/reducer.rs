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

            SummarizeIntent::Success => SummarizeDialogState::Hidden,

            SummarizeIntent::Error { message } => {
                // Determine current attempt number
                let current_attempt = match &state {
                    SummarizeDialogState::Summarizing { .. } => 1,
                    SummarizeDialogState::Retrying { attempt, .. } => attempt + 1,
                    _ => 1,
                };

                if current_attempt >= MAX_AUTO_RETRIES {
                    // Max retries reached, show user choice
                    SummarizeDialogState::Failed { error: message, selected_button: 0 }
                } else {
                    // Auto-retry
                    SummarizeDialogState::Retrying {
                        attempt: current_attempt,
                        error: message,
                        animation_tick: 0,
                    }
                }
            }

            SummarizeIntent::ToggleButton => match state {
                SummarizeDialogState::Failed { error, selected_button } => {
                    SummarizeDialogState::Failed {
                        error,
                        selected_button: if selected_button == 0 { 1 } else { 0 },
                    }
                }
                other => other,
            },

            SummarizeIntent::Hide => SummarizeDialogState::Hidden,
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
            SummarizeDialogState::Failed { error, selected_button } => {
                assert_eq!(error, "final error");
                assert_eq!(selected_button, 0);
            }
            _ => panic!("Expected Failed state"),
        }
    }

    #[test]
    fn success_transitions_to_hidden() {
        let state = SummarizeDialogState::Summarizing { animation_tick: 0 };
        let new_state = SummarizeReducer::reduce(state, SummarizeIntent::Success);
        assert_eq!(new_state, SummarizeDialogState::Hidden);
    }
}
