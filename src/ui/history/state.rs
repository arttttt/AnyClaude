use crate::ui::mvi::UiState;
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

impl UiState for HistoryDialogState {}

impl HistoryDialogState {
    pub fn is_visible(&self) -> bool {
        !matches!(self, Self::Hidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_is_default() {
        assert_eq!(HistoryDialogState::default(), HistoryDialogState::Hidden);
    }

    #[test]
    fn is_visible_check() {
        assert!(!HistoryDialogState::Hidden.is_visible());
        assert!(HistoryDialogState::Visible {
            entries: vec![],
            scroll_offset: 0,
        }
        .is_visible());
    }
}
