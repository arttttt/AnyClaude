use crate::config::SettingsFieldSnapshot;
use mvi::UiState;

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

impl UiState for SettingsDialogState {}

impl SettingsDialogState {
    pub fn is_visible(&self) -> bool {
        !matches!(self, Self::Hidden)
    }
}
