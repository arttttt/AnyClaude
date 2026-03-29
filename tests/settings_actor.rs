mod common;

use anyclaude::config::{SettingId, SettingSection, SettingsFieldSnapshot};
use anyclaude::ui::settings::{SettingsActor, SettingsDialogState, SettingsIntent};
use mvi::Store;

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

fn store() -> Store<SettingsActor> {
    Store::new(SettingsActor, |_| {})
}

fn store_with(state: SettingsDialogState) -> Store<SettingsActor> {
    Store::with_state(state, SettingsActor, |_| {})
}

#[test]
fn load_shows_dialog() {
    let mut store = store();
    store.dispatch(SettingsIntent::Load {
        fields: make_fields(),
    });
    assert!(store.state().is_visible());
}

#[test]
fn load_sets_focused_zero_and_not_dirty() {
    let mut store = store();
    store.dispatch(SettingsIntent::Load {
        fields: make_fields(),
    });
    if let SettingsDialogState::Visible {
        focused, dirty, confirm_discard, ..
    } = store.state()
    {
        assert_eq!(*focused, 0);
        assert!(!dirty);
        assert!(!confirm_discard);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn close_hides_dialog() {
    let mut store = store_with(make_visible(false));
    store.dispatch(SettingsIntent::Close);
    assert!(!store.state().is_visible());
}

#[test]
fn toggle_inverts_value_and_sets_dirty() {
    let mut store = store_with(make_visible(false));
    store.dispatch(SettingsIntent::Toggle);
    if let SettingsDialogState::Visible {
        fields, dirty, ..
    } = store.state()
    {
        assert!(fields[0].value);
        assert!(dirty);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn toggle_twice_reverts_value() {
    let mut store = store_with(make_visible(false));
    store.dispatch(SettingsIntent::Toggle);
    store.dispatch(SettingsIntent::Toggle);
    if let SettingsDialogState::Visible { fields, .. } = store.state() {
        assert!(!fields[0].value);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn move_down_wraps_around() {
    let mut store = store_with(make_visible(false));
    store.dispatch(SettingsIntent::MoveDown);
    if let SettingsDialogState::Visible { focused, .. } = store.state() {
        assert_eq!(*focused, 0);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn move_up_wraps_around() {
    let mut store = store_with(make_visible(false));
    store.dispatch(SettingsIntent::MoveUp);
    if let SettingsDialogState::Visible { focused, .. } = store.state() {
        assert_eq!(*focused, 0);
    } else {
        panic!("expected Visible");
    }
}

#[test]
fn move_on_hidden_is_noop() {
    let mut store = store();
    store.dispatch(SettingsIntent::MoveDown);
    assert!(!store.state().is_visible());
}

#[test]
fn toggle_on_hidden_is_noop() {
    let mut store = store();
    store.dispatch(SettingsIntent::Toggle);
    assert!(!store.state().is_visible());
}

#[test]
fn request_close_when_clean_hides_dialog() {
    let mut store = store_with(make_visible(false));
    store.dispatch(SettingsIntent::RequestClose);
    assert!(!store.state().is_visible());
}

#[test]
fn request_close_when_dirty_sets_confirm_discard() {
    let mut store = store_with(make_visible(false));
    store.dispatch(SettingsIntent::Toggle);
    store.dispatch(SettingsIntent::RequestClose);
    assert!(store.state().is_visible(), "should stay visible on first Escape");
    if let SettingsDialogState::Visible { confirm_discard, .. } = store.state() {
        assert!(confirm_discard, "confirm_discard should be true");
    }
}

#[test]
fn request_close_second_escape_hides_dialog() {
    let mut store = store_with(make_visible(false));
    store.dispatch(SettingsIntent::Toggle);
    store.dispatch(SettingsIntent::RequestClose);
    assert!(store.state().is_visible());
    store.dispatch(SettingsIntent::RequestClose);
    assert!(!store.state().is_visible());
}

#[test]
fn toggle_after_confirm_discard_resets_flag() {
    let mut store = store_with(make_visible(false));
    store.dispatch(SettingsIntent::Toggle);
    store.dispatch(SettingsIntent::RequestClose);
    store.dispatch(SettingsIntent::Toggle);
    if let SettingsDialogState::Visible { confirm_discard, .. } = store.state() {
        assert!(!confirm_discard, "toggle should reset confirm_discard");
    }
}

#[test]
fn request_close_on_hidden_is_noop() {
    let mut store = store();
    store.dispatch(SettingsIntent::RequestClose);
    assert!(!store.state().is_visible());
}
