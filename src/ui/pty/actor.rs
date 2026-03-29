//! Actor for the PTY lifecycle.

use std::collections::VecDeque;

use mvi::{Actor, ActorScope};

use super::intent::PtyIntent;
use super::state::PtyLifecycleState;

/// Side effects emitted by the PTY actor.
pub enum PtySideEffect {
    /// Flush buffered input to the PTY now that Claude Code is ready.
    FlushBuffer(VecDeque<Vec<u8>>),
}

pub struct PtyActor;

impl Actor for PtyActor {
    type State = PtyLifecycleState;
    type Intent = PtyIntent;
    type SideEffect = PtySideEffect;

    fn handle_intent(
        &self,
        intent: Self::Intent,
        scope: &mut ActorScope<Self::State, Self::SideEffect>,
    ) {
        match intent {
            PtyIntent::Attach => {
                scope.reduce(|state| match state {
                    PtyLifecycleState::Pending { buffer } => {
                        PtyLifecycleState::Attached { buffer }
                    }
                    PtyLifecycleState::Attached { buffer } => {
                        PtyLifecycleState::Attached { buffer }
                    }
                    PtyLifecycleState::Restarting => PtyLifecycleState::Attached {
                        buffer: VecDeque::new(),
                    },
                    PtyLifecycleState::Ready => PtyLifecycleState::Ready,
                });
            }
            PtyIntent::GotOutput => {
                let mut extracted = VecDeque::new();
                scope.reduce(|state| match state {
                    PtyLifecycleState::Attached { buffer } => {
                        extracted = buffer;
                        PtyLifecycleState::Ready
                    }
                    other => other,
                });
                if !extracted.is_empty() {
                    scope.side_effect(PtySideEffect::FlushBuffer(extracted));
                }
            }
            PtyIntent::BufferInput { bytes } => {
                scope.reduce(|state| match state {
                    PtyLifecycleState::Pending { mut buffer } => {
                        buffer.push_back(bytes);
                        PtyLifecycleState::Pending { buffer }
                    }
                    PtyLifecycleState::Attached { mut buffer } => {
                        buffer.push_back(bytes);
                        PtyLifecycleState::Attached { buffer }
                    }
                    PtyLifecycleState::Ready => PtyLifecycleState::Ready,
                    PtyLifecycleState::Restarting => PtyLifecycleState::Restarting,
                });
            }
            PtyIntent::Detach => {
                scope.reduce(|_| PtyLifecycleState::Restarting);
            }
            PtyIntent::SpawnFailed => {
                scope.reduce(|state| match state {
                    PtyLifecycleState::Restarting => PtyLifecycleState::Pending {
                        buffer: VecDeque::new(),
                    },
                    other => other,
                });
            }
        }
    }
}
