//! Unit coverage for the panel overlay collapse/expand epoch — pure step/derive
//! logic, no window. Mirrors the popup-fade test discipline.

use std::time::{Duration, Instant};

use anyclaude::ui::panel_anim::{panel_width_factor, step_panel_anim, PanelAnim};

const SECS: f32 = 0.14;

#[test]
fn no_epoch_steady_states() {
    let now = Instant::now();
    // No transition: collapsed → factor 0, expanded → factor 1, neither animating.
    assert_eq!(panel_width_factor(None, false, now, SECS), (0.0, false));
    assert_eq!(panel_width_factor(None, true, now, SECS), (1.0, false));
}

#[test]
fn step_arms_on_visibility_edges() {
    let now = Instant::now();
    // Collapsed + hidden → no epoch.
    assert_eq!(step_panel_anim(None, false, now), None);
    // Hidden → visible arms an expand.
    let a = step_panel_anim(None, true, now).unwrap();
    assert!(a.expanding);
    // Expanding → hidden flips to a collapse, restarting the clock.
    let later = now + Duration::from_millis(50);
    let b = step_panel_anim(Some(a), false, later).unwrap();
    assert!(!b.expanding);
    assert_eq!(b.started_at, later);
    // Mid-collapse reversal → expand again.
    let c = step_panel_anim(Some(b), true, later).unwrap();
    assert!(c.expanding);
}

#[test]
fn step_holds_steady_state() {
    let now = Instant::now();
    let a = PanelAnim { started_at: now, expanding: true };
    // Expanding + still visible → unchanged (no edge).
    assert_eq!(step_panel_anim(Some(a), true, now + Duration::from_millis(10)), Some(a));
}

#[test]
fn expanding_factor_ramps_zero_to_one() {
    let t0 = Instant::now();
    let a = Some(PanelAnim { started_at: t0, expanding: true });
    let (f0, anim0) = panel_width_factor(a, true, t0, SECS);
    assert!(f0.abs() < 1e-4, "starts at 0, got {f0}");
    assert!(anim0, "still animating at t=0");

    let mid = t0 + Duration::from_secs_f32(SECS / 2.0);
    let (fm, _) = panel_width_factor(a, true, mid, SECS);
    assert!((fm - 0.5).abs() < 1e-3, "symmetric ease midpoint ~0.5, got {fm}");

    let end = t0 + Duration::from_secs_f32(SECS);
    let (f1, anim1) = panel_width_factor(a, true, end, SECS);
    assert!((f1 - 1.0).abs() < 1e-4, "ends at 1, got {f1}");
    assert!(!anim1, "done animating at t>=1");
}

#[test]
fn collapsing_factor_ramps_one_to_zero() {
    let t0 = Instant::now();
    let a = Some(PanelAnim { started_at: t0, expanding: false });
    let (f0, _) = panel_width_factor(a, false, t0, SECS);
    assert!((f0 - 1.0).abs() < 1e-4, "collapse starts at full width, got {f0}");

    let end = t0 + Duration::from_secs_f32(SECS);
    let (f1, anim1) = panel_width_factor(a, false, end, SECS);
    assert!(f1.abs() < 1e-4, "collapse ends at 0, got {f1}");
    assert!(!anim1);
}
