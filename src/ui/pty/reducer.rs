//! Reducer for the PTY lifecycle.

use std::collections::VecDeque;

use mvi::Reducer;

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
                PtyLifecycleState::Restarting => {
                    // New PTY attached after restart — fresh buffer
                    PtyLifecycleState::Attached {
                        buffer: VecDeque::new(),
                    }
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
                // Drop input during restart and when ready
                PtyLifecycleState::Ready => PtyLifecycleState::Ready,
                PtyLifecycleState::Restarting => PtyLifecycleState::Restarting,
            },

            PtyIntent::Detach => PtyLifecycleState::Restarting,

            PtyIntent::SpawnFailed => match state {
                PtyLifecycleState::Restarting => PtyLifecycleState::Pending {
                    buffer: VecDeque::new(),
                },
                other => other,
            },
        }
    }
}
