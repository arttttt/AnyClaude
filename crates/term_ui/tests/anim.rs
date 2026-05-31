//! Unit coverage for the animation engine: `Interpolator`, `Animatable`, and the
//! generic `Animation<T>` tween. Pure, clock-driven; no GPU.

use std::time::{Duration, Instant};

use term_ui::{Animatable, Animation, Interpolator};

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn interpolator_endpoints_and_clamp() {
    for interp in [
        Interpolator::Linear,
        Interpolator::EaseIn,
        Interpolator::EaseOut,
        Interpolator::EaseInOut,
        Interpolator::CubicBezier { x1: 0.25, y1: 0.1, x2: 0.25, y2: 1.0 },
    ] {
        assert!(approx(interp.interpolate(0.0), 0.0), "{interp:?} at 0");
        assert!(approx(interp.interpolate(1.0), 1.0), "{interp:?} at 1");
        // Out-of-range input is clamped to the endpoints.
        assert!(approx(interp.interpolate(-5.0), 0.0));
        assert!(approx(interp.interpolate(5.0), 1.0));
    }
    // EaseInOut is symmetric about the midpoint.
    assert!(approx(Interpolator::EaseInOut.interpolate(0.5), 0.5));
}

#[test]
fn cubic_bezier_diagonal_is_linear() {
    // Control points on the diagonal reproduce the identity curve.
    let lin = Interpolator::CubicBezier { x1: 1.0 / 3.0, y1: 1.0 / 3.0, x2: 2.0 / 3.0, y2: 2.0 / 3.0 };
    for &t in &[0.0, 0.2, 0.5, 0.75, 1.0] {
        assert!(approx(lin.interpolate(t), t), "bezier-diagonal at {t}");
    }
}

#[test]
fn custom_interpolator_runs() {
    let sq = Interpolator::Custom(|t| t * t);
    assert!(approx(sq.interpolate(0.5), 0.25));
}

#[test]
fn animatable_lerp_f32_and_color() {
    assert!(approx(f32::lerp(0.0, 10.0, 0.25), 2.5));
    let c = <[f32; 4]>::lerp([0.0, 0.0, 0.0, 1.0], [1.0, 0.5, 0.0, 1.0], 0.5);
    assert!(approx(c[0], 0.5) && approx(c[1], 0.25) && approx(c[2], 0.0) && approx(c[3], 1.0));
}

#[test]
fn settled_is_not_animating() {
    let now = Instant::now();
    let a = Animation::settled(5.0_f32, now, Duration::from_secs_f32(0.2), Interpolator::Linear);
    // Even within the duration window, a settled tween holds its value.
    assert_eq!(a.value(now), 5.0);
    assert_eq!(a.value(now + Duration::from_millis(50)), 5.0);
    assert!(!a.animating(now + Duration::from_millis(50)));
    assert_eq!(a.target(), 5.0);
}

#[test]
fn retarget_ramps_and_settles() {
    let dur = Duration::from_secs_f32(0.2);
    let t0 = Instant::now();
    let mut a = Animation::settled(0.0_f32, t0, dur, Interpolator::Linear);
    a.retarget(1.0, t0);
    assert!(approx(a.value(t0), 0.0));
    assert!(a.animating(t0));
    assert!(approx(a.value(t0 + dur / 2), 0.5), "linear midpoint");
    assert!(approx(a.value(t0 + dur), 1.0));
    assert!(!a.animating(t0 + dur), "done at t>=1");
}

#[test]
fn retarget_is_idempotent_for_same_target() {
    let dur = Duration::from_secs_f32(0.2);
    let t0 = Instant::now();
    let mut a = Animation::settled(0.0_f32, t0, dur, Interpolator::Linear);
    a.retarget(1.0, t0);
    let mid = t0 + dur / 2;
    let v = a.value(mid);
    // Re-aiming at the SAME target must not restart the tween.
    a.retarget(1.0, mid);
    assert!(approx(a.value(mid), v), "no restart on same target");
}

#[test]
fn reversal_does_not_jump() {
    let dur = Duration::from_secs_f32(0.2);
    let t0 = Instant::now();
    let mut a = Animation::settled(0.0_f32, t0, dur, Interpolator::Linear);
    a.retarget(1.0, t0);
    let mid = t0 + dur / 2;
    let v = a.value(mid); // ~0.5
    // Reverse toward 0 mid-flight: value is continuous (rebased from `v`).
    a.retarget(0.0, mid);
    assert!(approx(a.value(mid), v), "reversal rebases from the current value");
    assert!(approx(a.value(mid + dur), 0.0), "settles at the new target");
}

#[test]
fn snap_is_instant() {
    let dur = Duration::from_secs_f32(0.2);
    let t0 = Instant::now();
    let mut a = Animation::settled(0.0_f32, t0, dur, Interpolator::EaseOut);
    a.retarget(1.0, t0);
    a.snap(0.5);
    assert_eq!(a.value(t0), 0.5);
    assert_eq!(a.value(t0 + dur), 0.5);
    assert!(!a.animating(t0), "a snapped tween is settled");
}
