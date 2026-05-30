//! AppState — the single bucket-1 truth (Phase E.2). Pins the cross-cutting
//! popup gate / close-all and the derived session-flash + uptime (R12: derived,
//! never stored). The per-popup transition logic is covered by the popup
//! suites; this exercises what AppState adds on top.

use std::time::{Duration, Instant};

use anyclaude::ui::app_state::{ApplyCtx, AppState, Effect, Msg};
use anyclaude::ui::backend_switch::BackendSwitchIntent;
use anyclaude::ui::history::HistoryIntent;
use anyclaude::ui::settings::SettingsIntent;
use glam::Vec2;
use term_core::{create_emulator, RenderSnapshot};
use term_gpu::{CellPoint, ScrollVelocity};
use winit::event::TouchPhase;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

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

// ── keyboard routing through apply (E.8.3) ───────────────────────────────

fn ctx() -> ApplyCtx<'static> {
    ApplyCtx { now: Instant::now(), snapshot: None, multi_click_threshold_ms: 400 }
}

/// A key Msg with a dummy logical key (the popup / shortcut paths read only the
/// physical code).
fn key(physical: KeyCode) -> Msg {
    Msg::Key {
        logical: Key::Named(NamedKey::Space),
        logical_unmod: Key::Named(NamedKey::Space),
        physical: PhysicalKey::Code(physical),
        app_cursor: false,
    }
}

#[test]
fn terminal_key_emits_write_to_pty() {
    // No popup, no Super: a plain key encodes to PTY bytes.
    let mut s = state();
    let fx = s.apply(
        Msg::Key {
            logical: Key::Named(NamedKey::Enter),
            logical_unmod: Key::Named(NamedKey::Enter),
            physical: PhysicalKey::Code(KeyCode::Enter),
            app_cursor: false,
        },
        &ctx(),
    );
    assert!(matches!(fx.as_slice(), [Effect::WriteToPty(_)]), "terminal key writes to PTY: {fx:?}");
}

#[test]
fn super_shortcut_maps_to_its_effect() {
    let mut s = state();
    s.modifiers = ModifiersState::SUPER;
    assert_eq!(s.apply(key(KeyCode::KeyQ), &ctx()), vec![Effect::Quit]);
    assert_eq!(s.apply(key(KeyCode::KeyB), &ctx()), vec![Effect::ToggleBackendPopup]);
    // A bare Super press with no mapped shortcut produces nothing.
    assert!(s.apply(key(KeyCode::F13), &ctx()).is_empty());
}

#[test]
fn popup_escape_closes_and_redraws() {
    let mut s = state();
    s.backend_switch.apply(BackendSwitchIntent::Open {
        backend_selection: 0,
        subagent_selection: 0,
        teammate_selection: 0,
        backends_count: 2,
    });
    assert_eq!(s.apply(key(KeyCode::Escape), &ctx()), vec![Effect::Redraw]);
    assert!(!s.any_popup_visible(), "Esc closes the backend popup");
}

#[test]
fn popup_enter_applies_then_closes_via_effects() {
    let mut s = state();
    s.backend_switch.apply(BackendSwitchIntent::Open {
        backend_selection: 0,
        subagent_selection: 0,
        teammate_selection: 0,
        backends_count: 2,
    });
    let fx = s.apply(key(KeyCode::Enter), &ctx());
    assert_eq!(fx, vec![Effect::ApplyBackendSelection, Effect::ClosePopups, Effect::Redraw]);
    // apply emits ClosePopups but does NOT close the popup itself — perform
    // runs the effects (ApplyBackendSelection reads the still-visible selection
    // before ClosePopups hides it).
    assert!(s.any_popup_visible());
}

#[test]
fn popup_nav_redraws_and_keys_pass_to_the_popup() {
    let mut s = state();
    s.backend_switch.apply(BackendSwitchIntent::Open {
        backend_selection: 0,
        subagent_selection: 0,
        teammate_selection: 0,
        backends_count: 3,
    });
    // A nav key moves within the popup and redraws (not a terminal key — the
    // popup owns input while open, so no WriteToPty escapes to the PTY).
    assert_eq!(s.apply(key(KeyCode::ArrowDown), &ctx()), vec![Effect::Redraw]);
    // An unmapped key while the popup is open is swallowed (no effect).
    assert!(s.apply(key(KeyCode::KeyZ), &ctx()).is_empty());
}

// ── mouse routing through apply (E.8.4) ──────────────────────────────────

fn press(
    in_header: bool,
    in_session_zone: bool,
    mouse_report: Option<Vec<u8>>,
    point: Option<CellPoint>,
) -> Msg {
    Msg::MousePress { in_header, in_session_zone, point, mouse_report }
}

#[test]
fn click_on_open_popup_dismisses_it() {
    let mut s = state();
    s.history.apply(HistoryIntent::Load { entries: vec![] });
    assert_eq!(s.apply(press(false, false, None, None), &ctx()), vec![Effect::Redraw]);
    assert!(!s.any_popup_visible(), "a click anywhere dismisses the open popup");
}

#[test]
fn header_session_click_copies_the_id() {
    let mut s = state();
    assert_eq!(s.apply(press(true, true, None, None), &ctx()), vec![Effect::CopySessionId]);
}

#[test]
fn header_click_outside_the_session_zone_does_nothing() {
    let mut s = state();
    assert!(s.apply(press(true, false, None, None), &ctx()).is_empty());
}

#[test]
fn mouse_reporting_app_gets_the_press_forwarded_not_a_selection() {
    let mut s = state();
    // A mouse-reporting app's encoded press is forwarded; selection is suppressed.
    let fx = s.apply(
        press(false, false, Some(b"\x1b[<0;2;2M".to_vec()), Some(CellPoint { row: 1, col: 1 })),
        &ctx(),
    );
    assert!(matches!(fx.as_slice(), [Effect::WriteToPty(_)]), "forwarded, not selected: {fx:?}");
    assert!(s.selection.is_none(), "selection must not shadow a mouse-reporting app");
}

// ── resize / tick / close / pty / modifiers routing through apply (E.8.2/5) ──

#[test]
fn grid_resized_updates_grid_and_asks_to_resize_resources() {
    let mut s = state(); // starts (80, 24)
    let fx = s.apply(Msg::GridResized { cols: 100, rows: 40 }, &ctx());
    assert_eq!(s.grid_size, (100, 40));
    assert_eq!(
        fx,
        vec![Effect::ResizeEmulatorAndPty { cols: 100, rows: 40 }, Effect::Redraw]
    );
}

#[test]
fn grid_resized_to_the_same_size_only_redraws() {
    let mut s = state(); // (80, 24)
    assert_eq!(s.apply(Msg::GridResized { cols: 80, rows: 24 }, &ctx()), vec![Effect::Redraw]);
}

#[test]
fn tick_close_pty_map_to_their_effects() {
    let mut s = state();
    assert_eq!(s.apply(Msg::Tick, &ctx()), vec![Effect::Redraw]);
    assert_eq!(s.apply(Msg::Close, &ctx()), vec![Effect::Quit]);
    assert_eq!(s.apply(Msg::PtyBytes, &ctx()), vec![Effect::Drain]);
}

#[test]
fn modifiers_changed_updates_state_with_no_effect() {
    let mut s = state();
    assert!(s.apply(Msg::ModifiersChanged(ModifiersState::SUPER), &ctx()).is_empty());
    assert!(s.modifiers.super_key());
}
