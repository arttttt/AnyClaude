//! AppState — the single bucket-1 truth (Phase E.2). Pins the cross-cutting
//! popup gate / close-all and the derived session-flash + uptime (R12: derived,
//! never stored). The per-popup transition logic is covered by the popup
//! suites; this exercises what AppState adds on top.

use std::time::{Duration, Instant};

use anyclaude::ui::app_state::AppState;
use anyclaude::ui::backend_switch::BackendSwitchIntent;
use anyclaude::ui::history::HistoryIntent;
use anyclaude::ui::settings::SettingsIntent;

fn state() -> AppState {
    AppState::new("session-abc".to_string(), Instant::now(), (80, 24))
}

#[test]
fn fresh_state_has_no_visible_popup() {
    assert!(!state().any_popup_visible());
}

#[test]
fn any_popup_visible_tracks_each_popup() {
    let mut s = state();
    s.backend_switch.apply(BackendSwitchIntent::Open {
        backend_selection: 0,
        subagent_selection: 0,
        teammate_selection: 0,
        backends_count: 1,
    });
    assert!(s.any_popup_visible());

    let mut s = state();
    s.history.apply(HistoryIntent::Load { entries: vec![] });
    assert!(s.any_popup_visible());

    let mut s = state();
    s.settings.apply(SettingsIntent::Load { fields: vec![] });
    assert!(s.any_popup_visible());
}

#[test]
fn close_all_popups_hides_everything() {
    let mut s = state();
    s.backend_switch.apply(BackendSwitchIntent::Open {
        backend_selection: 0,
        subagent_selection: 0,
        teammate_selection: 0,
        backends_count: 1,
    });
    s.history.apply(HistoryIntent::Load { entries: vec![] });
    s.settings.apply(SettingsIntent::Load { fields: vec![] });
    assert!(s.any_popup_visible());

    s.close_all_popups();
    assert!(!s.any_popup_visible());
    assert!(!s.backend_switch.is_visible());
    assert!(!s.history.is_visible());
    assert!(!s.settings.is_visible());
}

#[test]
fn session_copied_is_derived_from_deadline_and_frame_clock() {
    let mut s = state();
    let now = Instant::now();
    assert!(!s.session_copied(now), "unset → not copied");

    s.mark_session_copied(now + Duration::from_millis(1500));
    assert!(s.session_copied(now), "before the deadline → flashing");
    assert!(
        !s.session_copied(now + Duration::from_millis(1600)),
        "past the deadline → expired (no stored boolean to clear)"
    );
}

#[test]
fn uptime_is_derived_from_start_epoch() {
    let start = Instant::now();
    let s = AppState::new("s".to_string(), start, (80, 24));
    assert_eq!(s.uptime_secs(start + Duration::from_secs(5)), 5);
    assert_eq!(s.uptime_secs(start + Duration::from_millis(900)), 0);
}
