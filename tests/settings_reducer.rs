mod common;

use anyclaude::config::{SettingId, SettingSection, SettingsFieldSnapshot};
use mvi::Reducer;
use anyclaude::ui::settings::{SettingsDialogState, SettingsIntent, SettingsReducer};

fn make_fields() -> Vec<SettingsFieldSnapshot> {
    vec![
        SettingsFieldSnapshot {
            id: SettingId::Agents,
            label: "Agent Teams",
            description: "Enable multi-agent collaboration",
            section: SettingSection::Experimental,
            value: false,
        },
    ]
}

fn make_visible(dirty: bool) -> SettingsDialogState {
    SettingsDialogState::Visible {
        fields: make_fields(),
        focused: 0,
        dirty,
        confirm_discard: false,
    }
}

#[test]
fn load_shows_dialog() {
    let state = SettingsReducer::reduce(
        SettingsDialogState::Hidden,
        SettingsIntent::Load {
            fields: make_fields(),
        },
    );
    assert!(state.is_visible());
}

#[test]
fn load_sets_focused_zero_and_not_dirty() {
    let state = SettingsReducer::reduce(
        SettingsDialogState::Hidden,
        SettingsIntent::Load {
            fields: make_fields(),
        },
    );
    if let SettingsDialogState::Visible {
        focused, dirty, confirm_discard, ..
    } = state
    {
        assert_eq!(focused, 0);
        assert!(!dirty);
        assert!(!confirm_discard);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn close_hides_dialog() {
    let state = SettingsReducer::reduce(make_visible(false), SettingsIntent::Close);
    assert!(!state.is_visible());
}

#[test]
fn toggle_inverts_value_and_sets_dirty() {
    let state = SettingsReducer::reduce(make_visible(false), SettingsIntent::Toggle);
    if let SettingsDialogState::Visible {
        fields, dirty, ..
    } = state
    {
        assert!(fields[0].value);
        assert!(dirty);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn toggle_twice_reverts_value() {
    let state = make_visible(false);
    let state = SettingsReducer::reduce(state, SettingsIntent::Toggle);
    let state = SettingsReducer::reduce(state, SettingsIntent::Toggle);
    if let SettingsDialogState::Visible { fields, .. } = state {
        assert!(!fields[0].value);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn move_down_wraps_around() {
    let state = make_visible(false);
    // With 1 field, MoveDown should wrap to 0
    let state = SettingsReducer::reduce(state, SettingsIntent::MoveDown);
    if let SettingsDialogState::Visible { focused, .. } = state {
        assert_eq!(focused, 0);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn move_up_wraps_around() {
    let state = make_visible(false);
    // With 1 field, MoveUp should wrap to 0 (last = 0)
    let state = SettingsReducer::reduce(state, SettingsIntent::MoveUp);
    if let SettingsDialogState::Visible { focused, .. } = state {
        assert_eq!(focused, 0);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn move_on_hidden_is_noop() {
    let state = SettingsReducer::reduce(SettingsDialogState::Hidden, SettingsIntent::MoveDown);
    assert!(!state.is_visible());
}

#[test]
fn toggle_on_hidden_is_noop() {
    let state = SettingsReducer::reduce(SettingsDialogState::Hidden, SettingsIntent::Toggle);
    assert!(!state.is_visible());
}

// -- RequestClose (Escape with dirty confirmation) ----------------------------

#[test]
fn request_close_when_clean_hides_dialog() {
    let state = make_visible(false);
    let state = SettingsReducer::reduce(state, SettingsIntent::RequestClose);
    assert!(!state.is_visible());
}

#[test]
fn request_close_when_dirty_sets_confirm_discard() {
    let state = make_visible(false);
    // Toggle to make dirty
    let state = SettingsReducer::reduce(state, SettingsIntent::Toggle);
    // First Escape
    let state = SettingsReducer::reduce(state, SettingsIntent::RequestClose);
    assert!(state.is_visible(), "should stay visible on first Escape");
    if let SettingsDialogState::Visible { confirm_discard, .. } = state {
        assert!(confirm_discard, "confirm_discard should be true");
    }
}

#[test]
fn request_close_second_escape_hides_dialog() {
    let state = make_visible(false);
    let state = SettingsReducer::reduce(state, SettingsIntent::Toggle);
    // First Escape — sets confirm
    let state = SettingsReducer::reduce(state, SettingsIntent::RequestClose);
    assert!(state.is_visible());
    // Second Escape — closes
    let state = SettingsReducer::reduce(state, SettingsIntent::RequestClose);
    assert!(!state.is_visible());
}

#[test]
fn toggle_after_confirm_discard_resets_flag() {
    let state = make_visible(false);
    let state = SettingsReducer::reduce(state, SettingsIntent::Toggle);
    let state = SettingsReducer::reduce(state, SettingsIntent::RequestClose);
    // confirm_discard is true, now toggle again
    let state = SettingsReducer::reduce(state, SettingsIntent::Toggle);
    if let SettingsDialogState::Visible { confirm_discard, .. } = state {
        assert!(!confirm_discard, "toggle should reset confirm_discard");
    }
}

#[test]
fn request_close_on_hidden_is_noop() {
    let state = SettingsReducer::reduce(SettingsDialogState::Hidden, SettingsIntent::RequestClose);
    assert!(!state.is_visible());
}
