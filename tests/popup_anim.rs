//! `ui::popup_anim` — the pure popup fade epoch + alpha derivation, extracted
//! from the coordinator so it is headlessly testable (M2 from the E.7 review).
//! Covers all four `step_popup_anim` arms (open / close / reversal / hold) and
//! the alpha curve at t=0 / mid / >=1 in both directions, including the
//! `animating` flag flipping false at completion.

use std::time::{Duration, Instant};

use anyclaude::ui::popup_anim::{popup_fade_alpha, step_popup_anim, PopupAnim};

const FADE: f32 = 0.12;

#[test]
fn opens_on_first_visible_frame() {
    let now = Instant::now();
    let a = step_popup_anim(None, true, now).expect("opening epoch");
    assert!(a.opening, "first visible frame starts an opening fade");
    assert_eq!(a.started_at, now, "epoch stamped at the open edge");
}

#[test]
fn stays_none_while_no_popup() {
    assert_eq!(step_popup_anim(None, false, Instant::now()), None);
}

#[test]
fn holds_epoch_while_steady_open() {
    let now = Instant::now();
    let prev = Some(PopupAnim { started_at: now - Duration::from_millis(50), opening: true });
    assert_eq!(step_popup_anim(prev, true, now), prev, "steady open holds its epoch (no reset)");
}

#[test]
fn closes_on_the_hide_edge() {
    let now = Instant::now();
    let prev = Some(PopupAnim { started_at: now - Duration::from_millis(50), opening: true });
    let a = step_popup_anim(prev, false, now).expect("closing epoch");
    assert!(!a.opening, "hide edge flips to closing");
    assert_eq!(a.started_at, now, "epoch restamped at the close edge");
}

#[test]
fn holds_epoch_while_steady_closing() {
    let now = Instant::now();
    let prev = Some(PopupAnim { started_at: now - Duration::from_millis(50), opening: false });
    assert_eq!(step_popup_anim(prev, false, now), prev, "steady close holds its epoch");
}

#[test]
fn reverses_back_to_open_on_reopen_mid_close() {
    let now = Instant::now();
    let prev = Some(PopupAnim { started_at: now - Duration::from_millis(50), opening: false });
    let a = step_popup_anim(prev, true, now).expect("reopened epoch");
    assert!(a.opening, "reopening mid-close reverses to opening");
    assert_eq!(a.started_at, now, "epoch restamped at the reversal edge");
}

#[test]
fn no_anim_is_opaque_and_idle() {
    assert_eq!(popup_fade_alpha(None, Instant::now(), FADE), (1.0, false));
}

#[test]
fn opening_ramps_zero_to_one() {
    let t0 = Instant::now();
    let opening = Some(PopupAnim { started_at: t0, opening: true });

    let (a0, anim0) = popup_fade_alpha(opening, t0, FADE);
    assert!(a0.abs() < 1e-6 && anim0, "starts transparent, still animating: {a0}");

    let (amid, animmid) = popup_fade_alpha(opening, t0 + Duration::from_millis(60), FADE); // t=0.5
    assert!((amid - 0.875).abs() < 1e-3 && animmid, "ease_out(0.5)=0.875, animating: {amid}");

    let (a1, anim1) = popup_fade_alpha(opening, t0 + Duration::from_millis(200), FADE); // t>=1
    assert!((a1 - 1.0).abs() < 1e-6 && !anim1, "fully opaque + done: {a1}");
}

#[test]
fn closing_ramps_one_to_zero() {
    let t0 = Instant::now();
    let closing = Some(PopupAnim { started_at: t0, opening: false });

    let (a0, anim0) = popup_fade_alpha(closing, t0, FADE);
    assert!((a0 - 1.0).abs() < 1e-6 && anim0, "starts opaque, animating: {a0}");

    let (a1, anim1) = popup_fade_alpha(closing, t0 + Duration::from_millis(200), FADE); // t>=1
    assert!(a1.abs() < 1e-6 && !anim1, "fully transparent + done: {a1}");
}
