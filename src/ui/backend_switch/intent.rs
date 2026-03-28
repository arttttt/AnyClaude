use mvi::Intent;

#[derive(Debug, Clone)]
pub enum BackendSwitchIntent {
    Open {
        backend_selection: usize,
        subagent_selection: usize,
        teammate_selection: usize,
        backends_count: usize,
    },
    Close,
    NextSection,
    MoveUp,
    MoveDown,
}

impl Intent for BackendSwitchIntent {}
