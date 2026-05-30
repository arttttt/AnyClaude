//! The settings popup state machine, post-MVI: `SettingsDialogState::apply` is a
//! plain pure transition (no Store/Actor). Pins the ported semantics —
//! load/close, toggle dirties + inverts, wrap-around focus, and the dirty
//! discard-confirm flow (RequestClose arms on first Escape, hides on second,
//! and any edit re-arms it).

mod common;

use anyclaude::config::{SettingId, SettingSection, SettingsFieldSnapshot};
use anyclaude::ui::settings::{SettingsDialogState, SettingsIntent};

fn make_fields() -> Vec<SettingsFieldSnapshot> {
    vec![SettingsFieldSnapshot {
        id: SettingId::Agents,
        label: "Agent Teams",
        description: "Enable multi-agent collaboration",
        section: SettingSection::Experimental,
        value: false,
    }]
}

fn make_visible(dirty: bool) -> SettingsDialogState {
    SettingsDialogState::Visible {
        fields: make_fields(),
        focused: 0,
        dirty,
        confirm_discard: false,
    }
}

fn focused(s: &SettingsDialogState) -> usize {
    match s {
        SettingsDialogState::Visible { focused, .. } => *focused,
        SettingsDialogState::Hidden => panic!("expected Visible"),
    }
}

fn confirm_discard(s: &SettingsDialogState) -> bool {
    match s {
        SettingsDialogState::Visible { confirm_discard, .. } => *confirm_discard,
        SettingsDialogState::Hidden => panic!("expected Visible"),
    }
}

#[test]
fn load_shows_dialog() {
    let mut s = SettingsDialogState::default();
    s.apply(SettingsIntent::Load {
        fields: make_fields(),
    });
    assert!(s.is_visible());
}

#[test]
fn load_sets_focused_zero_and_not_dirty() {
    let mut s = SettingsDialogState::default();
    s.apply(SettingsIntent::Load {
        fields: make_fields(),
    });
    match s {
        SettingsDialogState::Visible {
            focused,
            dirty,
            confirm_discard,
            ..
        } => {
            assert_eq!(focused, 0);
            assert!(!dirty);
            assert!(!confirm_discard);
        }
        SettingsDialogState::Hidden => panic!("expected Visible"),
    }
}

#[test]
fn close_hides_dialog() {
    let mut s = make_visible(false);
    s.apply(SettingsIntent::Close);
    assert!(!s.is_visible());
}

#[test]
fn toggle_inverts_value_and_sets_dirty() {
    let mut s = make_visible(false);
    s.apply(SettingsIntent::Toggle);
    match &s {
        SettingsDialogState::Visible { fields, dirty, .. } => {
            assert!(fields[0].value);
            assert!(dirty);
        }
        SettingsDialogState::Hidden => panic!("expected Visible"),
    }
}

#[test]
fn toggle_twice_reverts_value() {
    let mut s = make_visible(false);
    s.apply(SettingsIntent::Toggle);
    s.apply(SettingsIntent::Toggle);
    match &s {
        SettingsDialogState::Visible { fields, .. } => assert!(!fields[0].value),
        SettingsDialogState::Hidden => panic!("expected Visible"),
    }
}

#[test]
fn move_down_wraps_around() {
    let mut s = make_visible(false);
    s.apply(SettingsIntent::MoveDown);
    assert_eq!(focused(&s), 0);
}

#[test]
fn move_up_wraps_around() {
    let mut s = make_visible(false);
    s.apply(SettingsIntent::MoveUp);
    assert_eq!(focused(&s), 0);
}

#[test]
fn move_on_hidden_is_noop() {
    let mut s = SettingsDialogState::default();
    s.apply(SettingsIntent::MoveDown);
    assert!(!s.is_visible());
}

#[test]
fn toggle_on_hidden_is_noop() {
    let mut s = SettingsDialogState::default();
    s.apply(SettingsIntent::Toggle);
    assert!(!s.is_visible());
}

#[test]
fn request_close_when_clean_hides_dialog() {
    let mut s = make_visible(false);
    s.apply(SettingsIntent::RequestClose);
    assert!(!s.is_visible());
}

#[test]
fn request_close_when_dirty_sets_confirm_discard() {
    let mut s = make_visible(false);
    s.apply(SettingsIntent::Toggle);
    s.apply(SettingsIntent::RequestClose);
    assert!(s.is_visible(), "should stay visible on first Escape");
    assert!(confirm_discard(&s), "confirm_discard should be true");
}

#[test]
fn request_close_second_escape_hides_dialog() {
    let mut s = make_visible(false);
    s.apply(SettingsIntent::Toggle);
    s.apply(SettingsIntent::RequestClose);
    assert!(s.is_visible());
    s.apply(SettingsIntent::RequestClose);
    assert!(!s.is_visible());
}

#[test]
fn toggle_after_confirm_discard_resets_flag() {
    let mut s = make_visible(false);
    s.apply(SettingsIntent::Toggle);
    s.apply(SettingsIntent::RequestClose);
    s.apply(SettingsIntent::Toggle);
    assert!(!confirm_discard(&s), "toggle should reset confirm_discard");
}

#[test]
fn request_close_on_hidden_is_noop() {
    let mut s = SettingsDialogState::default();
    s.apply(SettingsIntent::RequestClose);
    assert!(!s.is_visible());
}
