mod common;

use std::time::SystemTime;

use anyclaude::ui::history::{
    HistoryActor, HistoryDialogState, HistoryEntry, HistoryIntent, MAX_VISIBLE_ROWS,
};
use mvi::Store;

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

fn store() -> Store<HistoryActor> {
    Store::new(HistoryActor, |_| {})
}

fn store_with(state: HistoryDialogState) -> Store<HistoryActor> {
    Store::with_state(state, HistoryActor, |_| {})
}

#[test]
fn load_shows_dialog() {
    let mut store = store();
    store.dispatch(HistoryIntent::Load {
        entries: make_entries(3),
    });
    assert!(store.state().is_visible());
}

#[test]
fn load_scrolls_to_end() {
    let mut store = store();
    store.dispatch(HistoryIntent::Load {
        entries: make_entries(20),
    });
    if let HistoryDialogState::Visible { scroll_offset, .. } = store.state() {
        assert_eq!(*scroll_offset, 20 - MAX_VISIBLE_ROWS);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn close_hides_dialog() {
    let mut store = store_with(HistoryDialogState::Visible {
        entries: make_entries(3),
        scroll_offset: 0,
    });
    store.dispatch(HistoryIntent::Close);
    assert!(!store.state().is_visible());
}

#[test]
fn scroll_up_clamps_at_zero() {
    let mut store = store_with(HistoryDialogState::Visible {
        entries: make_entries(3),
        scroll_offset: 0,
    });
    store.dispatch(HistoryIntent::ScrollUp);
    if let HistoryDialogState::Visible { scroll_offset, .. } = store.state() {
        assert_eq!(*scroll_offset, 0);
    }
}

#[test]
fn scroll_down_clamps_at_max() {
    let entries = make_entries(20);
    let max = entries.len().saturating_sub(MAX_VISIBLE_ROWS);
    let mut store = store_with(HistoryDialogState::Visible {
        entries,
        scroll_offset: max,
    });
    store.dispatch(HistoryIntent::ScrollDown);
    if let HistoryDialogState::Visible { scroll_offset, .. } = store.state() {
        assert_eq!(*scroll_offset, max);
    }
}

#[test]
fn scroll_on_hidden_is_noop() {
    let mut store = store();
    store.dispatch(HistoryIntent::ScrollUp);
    assert!(!store.state().is_visible());
}
