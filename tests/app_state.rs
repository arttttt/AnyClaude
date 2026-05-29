//! AppState — the single bucket-1 truth (Phase E.2). Pins the cross-cutting
//! popup gate / close-all and the derived session-flash + uptime (R12: derived,
//! never stored). The per-popup transition logic is covered by the popup
//! suites; this exercises what AppState adds on top.

use std::time::{Duration, Instant};

use anyclaude::ui::app_state::{AppState, Effect};
use anyclaude::ui::backend_switch::BackendSwitchIntent;
use anyclaude::ui::history::HistoryIntent;
use anyclaude::ui::settings::SettingsIntent;
use glam::Vec2;
use term_core::{create_emulator, RenderSnapshot};
use term_gpu::{CellPoint, ScrollVelocity};
use winit::event::TouchPhase;

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

// ── scroll / momentum reducer (E.4) ──

/// A scrollable state: 1000px of content in a 500px viewport.
fn scrollable() -> AppState {
    let mut s = state();
    s.scroll.total_size_px = 1000.0;
    s.scroll.visible_px = 500.0;
    s
}

#[test]
fn wheel_cancels_then_redraws_and_records_velocity() {
    let mut s = scrollable();
    let fx = s.on_wheel(120.0, TouchPhase::Moved, true, Instant::now());
    // Precise (trackpad) → no silence-fallback; always cancels in-flight
    // momentum + pending fallback first, redraws last.
    assert_eq!(
        fx,
        vec![
            Effect::CancelMomentum,
            Effect::CancelGestureEnd,
            Effect::Redraw
        ]
    );
    assert!(s.scroll_velocity.is_some(), "wheel records velocity");
}

#[test]
fn non_precise_wheel_arms_gesture_end_fallback() {
    let mut s = scrollable();
    let fx = s.on_wheel(120.0, TouchPhase::Moved, false, Instant::now());
    assert!(fx.contains(&Effect::ScheduleGestureEnd));
}

#[test]
fn cancelled_wheel_drops_velocity() {
    let mut s = scrollable();
    let fx = s.on_wheel(120.0, TouchPhase::Cancelled, true, Instant::now());
    assert!(!fx.contains(&Effect::ScheduleMomentum));
    assert!(s.scroll_velocity.is_none());
}

#[test]
fn gesture_end_kicks_momentum_only_when_fast() {
    let now = Instant::now();

    // Fast → momentum scheduled, velocity retained (clamped).
    let mut fast = scrollable();
    fast.scroll_velocity = Some(ScrollVelocity {
        velocity: Vec2::new(0.0, 1.0e6),
        last_update: now,
    });
    assert_eq!(fast.on_gesture_end(now), vec![Effect::ScheduleMomentum]);
    assert!(fast.scroll_velocity.is_some());

    // Slow (zero) → no momentum, velocity dropped.
    let mut slow = scrollable();
    slow.scroll_velocity = Some(ScrollVelocity {
        velocity: Vec2::ZERO,
        last_update: now,
    });
    assert!(slow.on_gesture_end(now).is_empty());
    assert!(slow.scroll_velocity.is_none());
}

#[test]
fn momentum_tick_redraws_while_fast_and_stops_when_slow() {
    let base = Instant::now();

    // Still fast after a frame → redraw, velocity kept.
    let mut moving = scrollable();
    moving.scroll_velocity = Some(ScrollVelocity {
        velocity: Vec2::new(0.0, 1.0e6),
        last_update: base,
    });
    let fx = moving.on_momentum_tick(base + Duration::from_millis(16));
    assert_eq!(fx, vec![Effect::Redraw]);
    assert!(moving.scroll_velocity.is_some());

    // Below cutoff → cancel momentum, drop velocity, no redraw.
    let mut stopping = scrollable();
    stopping.scroll_velocity = Some(ScrollVelocity {
        velocity: Vec2::ZERO,
        last_update: base,
    });
    let fx = stopping.on_momentum_tick(base + Duration::from_millis(16));
    assert_eq!(fx, vec![Effect::CancelMomentum]);
    assert!(stopping.scroll_velocity.is_none());

    // No velocity → no-op.
    let mut idle = scrollable();
    assert!(idle.on_momentum_tick(base).is_empty());
}

// ── selection reducer (E.5) ──

/// A blank emulator snapshot — enough for the count-dispatch tests (word/line
/// boundary correctness is term_gpu's `expand_*` concern, not AppState's).
fn empty_snapshot() -> RenderSnapshot {
    create_emulator(20, 3, 100).snapshot()
}

#[test]
fn single_click_starts_a_linear_drag() {
    let mut s = state();
    let p = CellPoint { row: 0, col: 2 };
    s.begin_selection(p, 1, &empty_snapshot());
    assert!(s.dragging_selection, "single click keeps dragging");
    let sel = s.selection.expect("selection set");
    assert_eq!(sel.anchor, p);
    assert_eq!(sel.cursor, p);
}

#[test]
fn double_and_triple_click_snap_and_end_the_drag() {
    for count in [2, 3] {
        let mut s = state();
        s.begin_selection(CellPoint { row: 0, col: 2 }, count, &empty_snapshot());
        assert!(!s.dragging_selection, "word/line select does not drag (count {count})");
        assert!(s.selection.is_some());
    }
}

#[test]
fn drag_extends_only_an_active_selection() {
    let mut s = state();
    // Not dragging yet → no-op.
    assert!(!s.drag_selection_to(CellPoint { row: 1, col: 1 }));

    s.begin_selection(CellPoint { row: 0, col: 0 }, 1, &empty_snapshot());
    let to = CellPoint { row: 0, col: 5 };
    assert!(s.drag_selection_to(to), "active drag extends");
    assert_eq!(s.selection.unwrap().cursor, to);
}

#[test]
fn release_clears_a_click_without_drag_but_keeps_a_real_selection() {
    // Click with no drag → empty (anchor == cursor) → cleared on release.
    let mut empty = state();
    empty.begin_selection(CellPoint { row: 0, col: 0 }, 1, &empty_snapshot());
    assert!(empty.end_selection_drag(), "empty selection cleared");
    assert!(empty.selection.is_none());

    // Click then drag → non-empty → kept on release.
    let mut real = state();
    real.begin_selection(CellPoint { row: 0, col: 0 }, 1, &empty_snapshot());
    real.drag_selection_to(CellPoint { row: 0, col: 4 });
    assert!(!real.end_selection_drag(), "non-empty selection kept");
    assert!(real.selection.is_some());
}

#[test]
fn next_click_records_and_cycles() {
    let mut s = state();
    let p = CellPoint { row: 1, col: 1 };
    let now = Instant::now();
    assert_eq!(s.next_click(p, now, 400), 1);
    assert_eq!(s.next_click(p, now + Duration::from_millis(100), 400), 2);
    assert_eq!(s.next_click(p, now + Duration::from_millis(200), 400), 3);
    // Different cell resets.
    assert_eq!(s.next_click(CellPoint { row: 2, col: 2 }, now + Duration::from_millis(250), 400), 1);
}
