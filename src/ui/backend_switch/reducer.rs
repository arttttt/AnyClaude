use crate::ui::backend_switch::intent::BackendSwitchIntent;
use crate::ui::backend_switch::state::{BackendPopupSection, BackendSwitchState};
use mvi::Reducer;

pub struct BackendSwitchReducer;

impl Reducer for BackendSwitchReducer {
    type State = BackendSwitchState;
    type Intent = BackendSwitchIntent;

    fn reduce(state: Self::State, intent: Self::Intent) -> Self::State {
        match intent {
            BackendSwitchIntent::Open {
                backend_selection,
                subagent_selection,
                teammate_selection,
                backends_count,
            } => BackendSwitchState::Visible {
                section: BackendPopupSection::ActiveBackend,
                backend_selection,
                subagent_selection,
                teammate_selection,
                backends_count,
            },
            BackendSwitchIntent::Close => BackendSwitchState::Hidden,
            BackendSwitchIntent::NextSection => match state {
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
            },
            BackendSwitchIntent::MoveUp => navigate(state, -1),
            BackendSwitchIntent::MoveDown => navigate(state, 1),
        }
    }
}

fn navigate(state: BackendSwitchState, direction: i32) -> BackendSwitchState {
    match state {
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
                    let total = backends_count + 1; // 0 = Disabled
                    subagent_selection = wrap_around(subagent_selection, direction, total);
                }
                BackendPopupSection::TeammateBackend => {
                    let total = backends_count + 1; // 0 = Disabled
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
    }
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
