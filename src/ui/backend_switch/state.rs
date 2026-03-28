use mvi::UiState;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum BackendPopupSection {
    #[default]
    ActiveBackend,
    SubagentBackend,
    TeammateBackend,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum BackendSwitchState {
    #[default]
    Hidden,
    Visible {
        section: BackendPopupSection,
        backend_selection: usize,
        subagent_selection: usize,
        teammate_selection: usize,
        /// Number of backends (needed for wrap-around navigation).
        backends_count: usize,
    },
}

impl UiState for BackendSwitchState {}

impl BackendSwitchState {
    pub fn is_visible(&self) -> bool {
        !matches!(self, Self::Hidden)
    }
}
