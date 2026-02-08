mod common;

use anyclaude::ui::history::HistoryDialogState;

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
