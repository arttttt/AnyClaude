use crate::ui::history::state::HistoryEntry;

/// History popup intents — the message vocabulary consumed by
/// [`HistoryDialogState::apply`]. Plain enum (no MVI traits).
#[derive(Debug, Clone)]
pub enum HistoryIntent {
    Load { entries: Vec<HistoryEntry> },
    Close,
    ScrollUp,
    ScrollDown,
}
