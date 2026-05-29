//! The history popup state machine, post-MVI: `HistoryDialogState::apply` is a
//! plain pure transition (no Store/Actor). Folds the old `history_actor` +
//! `history_state` suites — load-scrolls-to-end, close, scroll clamps, and the
//! hidden/default + is_visible checks.

mod common;

use std::time::SystemTime;

use anyclaude::ui::history::{HistoryDialogState, HistoryEntry, HistoryIntent, MAX_VISIBLE_ROWS};

fn make_entries(count: usize) -> Vec<HistoryEntry> {
    (0..count)
        .map(|i| HistoryEntry {
            timestamp: SystemTime::now(),
            from_backend: if i == 0 {
                None
            } else {
                Some(format!("backend-{}", i - 1))
            },
            to_backend: format!("backend-{}", i),
        })
        .collect()
}

fn scroll_offset(s: &HistoryDialogState) -> usize {
    match s {
        HistoryDialogState::Visible { scroll_offset, .. } => *scroll_offset,
        HistoryDialogState::Hidden => panic!("expected Visible"),
    }
}

// ── state basics (folded from history_state) ──

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

// ── transitions (folded from history_actor) ──

#[test]
fn load_shows_dialog() {
    let mut s = HistoryDialogState::default();
    s.apply(HistoryIntent::Load {
        entries: make_entries(3),
    });
    assert!(s.is_visible());
}

#[test]
fn load_scrolls_to_end() {
    let mut s = HistoryDialogState::default();
    s.apply(HistoryIntent::Load {
        entries: make_entries(20),
    });
    assert_eq!(scroll_offset(&s), 20 - MAX_VISIBLE_ROWS);
}

#[test]
fn close_hides_dialog() {
    let mut s = HistoryDialogState::Visible {
        entries: make_entries(3),
        scroll_offset: 0,
    };
    s.apply(HistoryIntent::Close);
    assert!(!s.is_visible());
}

#[test]
fn scroll_up_clamps_at_zero() {
    let mut s = HistoryDialogState::Visible {
        entries: make_entries(3),
        scroll_offset: 0,
    };
    s.apply(HistoryIntent::ScrollUp);
    assert_eq!(scroll_offset(&s), 0);
}

#[test]
fn scroll_down_clamps_at_max() {
    let entries = make_entries(20);
    let max = entries.len().saturating_sub(MAX_VISIBLE_ROWS);
    let mut s = HistoryDialogState::Visible {
        entries,
        scroll_offset: max,
    };
    s.apply(HistoryIntent::ScrollDown);
    assert_eq!(scroll_offset(&s), max);
}

#[test]
fn scroll_on_hidden_is_noop() {
    let mut s = HistoryDialogState::default();
    s.apply(HistoryIntent::ScrollUp);
    assert!(!s.is_visible());
}
