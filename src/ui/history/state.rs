use mvi::State;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub timestamp: SystemTime,
    pub from_backend: Option<String>,
    pub to_backend: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum HistoryDialogState {
    #[default]
    Hidden,
    Visible {
        entries: Vec<HistoryEntry>,
        scroll_offset: usize,
    },
}

impl State for HistoryDialogState {}

impl HistoryDialogState {
    pub fn is_visible(&self) -> bool {
        !matches!(self, Self::Hidden)
    }
}
