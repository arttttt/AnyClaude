//! Reducer for the PTY lifecycle.

use crate::ui::mvi::Reducer;

use super::intent::PtyIntent;
use super::state::PtyLifecycleState;

/// Reducer for PTY lifecycle state transitions.
///
/// Pure function — all side effects (flushing buffer, sending input to PTY)
/// are handled by the caller around the dispatch call.
pub struct PtyReducer;

impl Reducer for PtyReducer {
    type State = PtyLifecycleState;
    type Intent = PtyIntent;

    fn reduce(state: Self::State, intent: Self::Intent) -> Self::State {
        match intent {
            PtyIntent::Attach => match state {
                PtyLifecycleState::Pending { buffer } => {
                    PtyLifecycleState::Attached { buffer }
                }
                PtyLifecycleState::Attached { buffer } => {
                    // Re-attach: keep existing buffer
                    PtyLifecycleState::Attached { buffer }
                }
                PtyLifecycleState::Ready => PtyLifecycleState::Ready,
            },

            PtyIntent::GotOutput => match state {
                PtyLifecycleState::Attached { .. } => PtyLifecycleState::Ready,
                other => other,
            },

            PtyIntent::BufferInput { bytes } => match state {
                PtyLifecycleState::Pending { mut buffer } => {
                    buffer.push_back(bytes);
                    PtyLifecycleState::Pending { buffer }
                }
                PtyLifecycleState::Attached { mut buffer } => {
                    buffer.push_back(bytes);
                    PtyLifecycleState::Attached { buffer }
                }
                PtyLifecycleState::Ready => PtyLifecycleState::Ready,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn pending_attach_transitions_to_attached() {
        let state = PtyLifecycleState::Pending {
            buffer: VecDeque::new(),
        };
        let new = PtyReducer::reduce(state, PtyIntent::Attach);
        assert!(matches!(new, PtyLifecycleState::Attached { buffer } if buffer.is_empty()));
    }

    #[test]
    fn attached_attach_preserves_buffer() {
        let mut buf = VecDeque::new();
        buf.push_back(b"hello".to_vec());
        let state = PtyLifecycleState::Attached { buffer: buf };

        let new = PtyReducer::reduce(state, PtyIntent::Attach);
        match new {
            PtyLifecycleState::Attached { buffer } => {
                assert_eq!(buffer.len(), 1);
                assert_eq!(buffer[0], b"hello");
            }
            _ => panic!("Expected Attached"),
        }
    }

    #[test]
    fn ready_attach_stays_ready() {
        let new = PtyReducer::reduce(PtyLifecycleState::Ready, PtyIntent::Attach);
        assert!(matches!(new, PtyLifecycleState::Ready));
    }

    #[test]
    fn attached_got_output_transitions_to_ready() {
        let state = PtyLifecycleState::Attached {
            buffer: VecDeque::new(),
        };
        let new = PtyReducer::reduce(state, PtyIntent::GotOutput);
        assert!(matches!(new, PtyLifecycleState::Ready));
    }

    #[test]
    fn pending_got_output_is_noop() {
        let state = PtyLifecycleState::Pending {
            buffer: VecDeque::new(),
        };
        let new = PtyReducer::reduce(state, PtyIntent::GotOutput);
        assert!(matches!(new, PtyLifecycleState::Pending { .. }));
    }

    #[test]
    fn ready_got_output_is_noop() {
        let new = PtyReducer::reduce(PtyLifecycleState::Ready, PtyIntent::GotOutput);
        assert!(matches!(new, PtyLifecycleState::Ready));
    }

    #[test]
    fn pending_buffer_input_appends() {
        let state = PtyLifecycleState::Pending {
            buffer: VecDeque::new(),
        };
        let new = PtyReducer::reduce(
            state,
            PtyIntent::BufferInput {
                bytes: b"data".to_vec(),
            },
        );
        match new {
            PtyLifecycleState::Pending { buffer } => {
                assert_eq!(buffer.len(), 1);
                assert_eq!(buffer[0], b"data");
            }
            _ => panic!("Expected Pending"),
        }
    }

    #[test]
    fn attached_buffer_input_appends() {
        let state = PtyLifecycleState::Attached {
            buffer: VecDeque::new(),
        };
        let new = PtyReducer::reduce(
            state,
            PtyIntent::BufferInput {
                bytes: b"data".to_vec(),
            },
        );
        match new {
            PtyLifecycleState::Attached { buffer } => {
                assert_eq!(buffer.len(), 1);
                assert_eq!(buffer[0], b"data");
            }
            _ => panic!("Expected Attached"),
        }
    }

    #[test]
    fn ready_buffer_input_is_noop() {
        let new = PtyReducer::reduce(
            PtyLifecycleState::Ready,
            PtyIntent::BufferInput {
                bytes: b"data".to_vec(),
            },
        );
        assert!(matches!(new, PtyLifecycleState::Ready));
    }

    #[test]
    fn multiple_buffer_inputs_accumulate() {
        let state = PtyLifecycleState::Pending {
            buffer: VecDeque::new(),
        };

        let state = PtyReducer::reduce(
            state,
            PtyIntent::BufferInput {
                bytes: b"first".to_vec(),
            },
        );
        let state = PtyReducer::reduce(
            state,
            PtyIntent::BufferInput {
                bytes: b"second".to_vec(),
            },
        );
        let state = PtyReducer::reduce(
            state,
            PtyIntent::BufferInput {
                bytes: b"third".to_vec(),
            },
        );

        match state {
            PtyLifecycleState::Pending { buffer } => {
                assert_eq!(buffer.len(), 3);
                assert_eq!(buffer[0], b"first");
                assert_eq!(buffer[1], b"second");
                assert_eq!(buffer[2], b"third");
            }
            _ => panic!("Expected Pending"),
        }
    }
}
