mod common;

use std::time::SystemTime;

use anyclaude::ui::history::{
    HistoryDialogState, HistoryEntry, HistoryIntent, HistoryReducer, MAX_VISIBLE_ROWS,
};
use anyclaude::ui::mvi::Reducer;

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

#[test]
fn load_shows_dialog() {
    let state = HistoryReducer::reduce(
        HistoryDialogState::Hidden,
        HistoryIntent::Load {
            entries: make_entries(3),
        },
    );
    assert!(state.is_visible());
}

#[test]
fn load_scrolls_to_end() {
    let entries = make_entries(20);
    let state = HistoryReducer::reduce(
        HistoryDialogState::Hidden,
        HistoryIntent::Load { entries },
    );
    if let HistoryDialogState::Visible { scroll_offset, .. } = state {
        assert_eq!(scroll_offset, 20 - MAX_VISIBLE_ROWS);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn close_hides_dialog() {
    let state = HistoryReducer::reduce(
        HistoryDialogState::Visible {
            entries: make_entries(3),
            scroll_offset: 0,
        },
        HistoryIntent::Close,
    );
    assert!(!state.is_visible());
}

#[test]
fn scroll_up_clamps_at_zero() {
    let state = HistoryDialogState::Visible {
        entries: make_entries(3),
        scroll_offset: 0,
    };
    let state = HistoryReducer::reduce(state, HistoryIntent::ScrollUp);
    if let HistoryDialogState::Visible { scroll_offset, .. } = state {
        assert_eq!(scroll_offset, 0);
    }
}

#[test]
fn scroll_down_clamps_at_max() {
    let entries = make_entries(20);
    let max = entries.len().saturating_sub(MAX_VISIBLE_ROWS);
    let state = HistoryDialogState::Visible {
        entries,
        scroll_offset: max,
    };
    let state = HistoryReducer::reduce(state, HistoryIntent::ScrollDown);
    if let HistoryDialogState::Visible { scroll_offset, .. } = state {
        assert_eq!(scroll_offset, max);
    }
}

#[test]
fn scroll_on_hidden_is_noop() {
    let state = HistoryReducer::reduce(HistoryDialogState::Hidden, HistoryIntent::ScrollUp);
    assert!(!state.is_visible());
}
