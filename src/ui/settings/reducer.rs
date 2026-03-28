use mvi::Reducer;
use crate::ui::settings::intent::SettingsIntent;
use crate::ui::settings::state::SettingsDialogState;

pub struct SettingsReducer;

impl Reducer for SettingsReducer {
    type State = SettingsDialogState;
    type Intent = SettingsIntent;

    fn reduce(state: Self::State, intent: Self::Intent) -> Self::State {
        match intent {
            SettingsIntent::Load { fields } => SettingsDialogState::Visible {
                fields,
                focused: 0,
                dirty: false,
                confirm_discard: false,
            },
            SettingsIntent::Close => SettingsDialogState::Hidden,
            SettingsIntent::RequestClose => match state {
                SettingsDialogState::Visible {
                    dirty: true,
                    confirm_discard: false,
                    fields,
                    focused,
                    ..
                } => {
                    // First Escape with unsaved changes: ask for confirmation
                    SettingsDialogState::Visible {
                        fields,
                        focused,
                        dirty: true,
                        confirm_discard: true,
                    }
                }
                _ => {
                    // Clean state or already confirming: close
                    SettingsDialogState::Hidden
                }
            },
            SettingsIntent::MoveUp => match state {
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
            },
            SettingsIntent::MoveDown => match state {
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
            },
            SettingsIntent::Toggle => match state {
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
            },
        }
    }
}
