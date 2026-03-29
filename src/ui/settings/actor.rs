use crate::ui::settings::intent::SettingsIntent;
use crate::ui::settings::state::SettingsDialogState;
use mvi::{Actor, ActorScope};

pub struct SettingsActor;

impl Actor for SettingsActor {
    type State = SettingsDialogState;
    type Intent = SettingsIntent;
    type SideEffect = ();

    fn handle_intent(
        &self,
        intent: Self::Intent,
        scope: &mut ActorScope<Self::State, Self::SideEffect>,
    ) {
        match intent {
            SettingsIntent::Load { fields } => {
                scope.reduce(|_| SettingsDialogState::Visible {
                    fields,
                    focused: 0,
                    dirty: false,
                    confirm_discard: false,
                });
            }
            SettingsIntent::Close => {
                scope.reduce(|_| SettingsDialogState::Hidden);
            }
            SettingsIntent::RequestClose => {
                scope.reduce(|state| match state {
                    SettingsDialogState::Visible {
                        dirty: true,
                        confirm_discard: false,
                        fields,
                        focused,
                        ..
                    } => SettingsDialogState::Visible {
                        fields,
                        focused,
                        dirty: true,
                        confirm_discard: true,
                    },
                    _ => SettingsDialogState::Hidden,
                });
            }
            SettingsIntent::MoveUp => {
                scope.reduce(|state| match state {
                    SettingsDialogState::Visible {
                        fields,
                        focused,
                        dirty,
                        ..
                    } => {
                        let new_focused = if focused == 0 {
                            fields.len().saturating_sub(1)
                        } else {
                            focused - 1
                        };
                        SettingsDialogState::Visible {
                            fields,
                            focused: new_focused,
                            dirty,
                            confirm_discard: false,
                        }
                    }
                    other => other,
                });
            }
            SettingsIntent::MoveDown => {
                scope.reduce(|state| match state {
                    SettingsDialogState::Visible {
                        fields,
                        focused,
                        dirty,
                        ..
                    } => {
                        let new_focused = if focused + 1 >= fields.len() {
                            0
                        } else {
                            focused + 1
                        };
                        SettingsDialogState::Visible {
                            fields,
                            focused: new_focused,
                            dirty,
                            confirm_discard: false,
                        }
                    }
                    other => other,
                });
            }
            SettingsIntent::Toggle => {
                scope.reduce(|state| match state {
                    SettingsDialogState::Visible {
                        mut fields,
                        focused,
                        ..
                    } => {
                        if let Some(field) = fields.get_mut(focused) {
                            field.value = !field.value;
                        }
                        SettingsDialogState::Visible {
                            fields,
                            focused,
                            dirty: true,
                            confirm_discard: false,
                        }
                    }
                    other => other,
                });
            }
        }
    }
}
