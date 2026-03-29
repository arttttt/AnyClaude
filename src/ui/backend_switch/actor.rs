use crate::ui::backend_switch::intent::BackendSwitchIntent;
use crate::ui::backend_switch::state::{BackendPopupSection, BackendSwitchState};
use mvi::{Actor, ActorScope};

pub struct BackendSwitchActor;

impl Actor for BackendSwitchActor {
    type State = BackendSwitchState;
    type Intent = BackendSwitchIntent;
    type SideEffect = ();

    fn handle_intent(
        &self,
        intent: Self::Intent,
        scope: &mut ActorScope<Self::State, Self::SideEffect>,
    ) {
        match intent {
            BackendSwitchIntent::Open {
                backend_selection,
                subagent_selection,
                teammate_selection,
                backends_count,
            } => {
                scope.reduce(|_| BackendSwitchState::Visible {
                    section: BackendPopupSection::ActiveBackend,
                    backend_selection,
                    subagent_selection,
                    teammate_selection,
                    backends_count,
                });
            }
            BackendSwitchIntent::Close => {
                scope.reduce(|_| BackendSwitchState::Hidden);
            }
            BackendSwitchIntent::NextSection => {
                scope.reduce(|state| match state {
                    BackendSwitchState::Visible {
                        section,
                        backend_selection,
                        subagent_selection,
                        teammate_selection,
                        backends_count,
                    } => {
                        let next = match section {
                            BackendPopupSection::ActiveBackend => {
                                BackendPopupSection::SubagentBackend
                            }
                            BackendPopupSection::SubagentBackend => {
                                BackendPopupSection::TeammateBackend
                            }
                            BackendPopupSection::TeammateBackend => {
                                BackendPopupSection::ActiveBackend
                            }
                        };
                        BackendSwitchState::Visible {
                            section: next,
                            backend_selection,
                            subagent_selection,
                            teammate_selection,
                            backends_count,
                        }
                    }
                    other => other,
                });
            }
            BackendSwitchIntent::MoveUp => navigate(scope, -1),
            BackendSwitchIntent::MoveDown => navigate(scope, 1),
        }
    }
}

fn navigate(scope: &mut ActorScope<BackendSwitchState, ()>, direction: i32) {
    scope.reduce(|state| match state {
        BackendSwitchState::Visible {
            section,
            mut backend_selection,
            mut subagent_selection,
            mut teammate_selection,
            backends_count,
        } => {
            match section {
                BackendPopupSection::ActiveBackend => {
                    backend_selection = wrap_around(backend_selection, direction, backends_count);
                }
                BackendPopupSection::SubagentBackend => {
                    let total = backends_count + 1;
                    subagent_selection = wrap_around(subagent_selection, direction, total);
                }
                BackendPopupSection::TeammateBackend => {
                    let total = backends_count + 1;
                    teammate_selection = wrap_around(teammate_selection, direction, total);
                }
            }
            BackendSwitchState::Visible {
                section,
                backend_selection,
                subagent_selection,
                teammate_selection,
                backends_count,
            }
        }
        other => other,
    });
}

fn wrap_around(current: usize, direction: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let current = current.min(len - 1);
    if direction.is_negative() {
        if current == 0 {
            len - 1
        } else {
            current - 1
        }
    } else if current + 1 >= len {
        0
    } else {
        current + 1
    }
}
