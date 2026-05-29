use crate::config::SettingsFieldSnapshot;
use crate::ui::settings::intent::SettingsIntent;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum SettingsDialogState {
    #[default]
    Hidden,
    Visible {
        fields: Vec<SettingsFieldSnapshot>,
        focused: usize,
        dirty: bool,
        /// When true, next Escape will discard changes. Set on first Escape when dirty.
        confirm_discard: bool,
    },
}

impl SettingsDialogState {
    pub fn is_visible(&self) -> bool {
        !matches!(self, Self::Hidden)
    }

    /// The single authoritative transition (the plain-fn replacement for the
    /// old MVI `Actor::handle_intent` — same semantics, mutated in place).
    pub fn apply(&mut self, intent: SettingsIntent) {
        match intent {
            SettingsIntent::Load { fields } => {
                *self = SettingsDialogState::Visible {
                    fields,
                    focused: 0,
                    dirty: false,
                    confirm_discard: false,
                };
            }
            SettingsIntent::Close => *self = SettingsDialogState::Hidden,
            SettingsIntent::RequestClose => {
                // First Escape on a dirty dialog arms the discard confirmation
                // (stays visible); a clean dialog, or a second Escape, hides it.
                let arm = matches!(
                    self,
                    SettingsDialogState::Visible {
                        dirty: true,
                        confirm_discard: false,
                        ..
                    }
                );
                if arm {
                    if let SettingsDialogState::Visible { confirm_discard, .. } = self {
                        *confirm_discard = true;
                    }
                } else {
                    *self = SettingsDialogState::Hidden;
                }
            }
            SettingsIntent::MoveUp => {
                if let SettingsDialogState::Visible {
                    fields,
                    focused,
                    confirm_discard,
                    ..
                } = self
                {
                    *focused = if *focused == 0 {
                        fields.len().saturating_sub(1)
                    } else {
                        *focused - 1
                    };
                    *confirm_discard = false;
                }
            }
            SettingsIntent::MoveDown => {
                if let SettingsDialogState::Visible {
                    fields,
                    focused,
                    confirm_discard,
                    ..
                } = self
                {
                    *focused = if *focused + 1 >= fields.len() {
                        0
                    } else {
                        *focused + 1
                    };
                    *confirm_discard = false;
                }
            }
            SettingsIntent::Toggle => {
                if let SettingsDialogState::Visible {
                    fields,
                    focused,
                    dirty,
                    confirm_discard,
                } = self
                {
                    if let Some(field) = fields.get_mut(*focused) {
                        field.value = !field.value;
                    }
                    *dirty = true;
                    *confirm_discard = false;
                }
            }
        }
    }
}
