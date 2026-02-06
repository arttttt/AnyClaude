use crate::ui::history::state::HistoryEntry;
use crate::ui::mvi::Intent;

#[derive(Debug, Clone)]
pub enum HistoryIntent {
    Load { entries: Vec<HistoryEntry> },
    Close,
    ScrollUp,
    ScrollDown,
}

impl Intent for HistoryIntent {}
