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
    /// Reset the current section's selection to "Disabled" (index 0).
    /// No-op in the Active section — the active backend cannot be
    /// cleared (the proxy always has one). Wired to Del / Backspace
    /// while the popup is open.
    Clear,
}

impl Intent for BackendSwitchIntent {}
