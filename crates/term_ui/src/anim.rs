//! Animation helpers (design §9/§12): pure easing curves + an overlay-alpha
//! bake. Transitions store NO resolved value (R12) — the coordinator derives a
//! transition's progress `t` from an epoch + the frame clock each frame, eases
//! it, and bakes the resulting alpha into a freshly-painted overlay
//! [`PaintOutput`] BEFORE merging it into the overlay layer. The bake is
//! non-incremental on purpose (§9): the whole overlay is re-emitted each
//! animating frame. Scoped to the emoji-free chrome/popup overlay — the
//! colour-glyph path ignores per-instance colour alpha.

use std::time::{Duration, Instant};

use glam::Vec2;

use crate::paint::PaintOutput;

/// An easing curve: maps normalized time `t ∈ [0, 1]` to eased progress. A
/// closed set of standard curves plus a `Custom(fn)` escape hatch, so a bespoke
/// curve needs no trait object or generics on [`Animation`]. The built-in
/// variants reuse the free `linear` / `ease_*` functions below.
#[derive(Debug, Clone, Copy)]
pub enum Interpolator {
    Linear,
    /// Cubic ease-in (accelerating) — the mirror of [`Interpolator::EaseOut`].
    EaseIn,
    /// Cubic ease-out (decelerating).
    EaseOut,
    /// Symmetric cubic ease-in-out.
    EaseInOut,
    /// CSS-style cubic Bézier through `(0,0)`, `(x1,y1)`, `(x2,y2)`, `(1,1)`.
    /// (e.g. `{1/3, 1/3, 2/3, 2/3}` is linear; `{0.25, 0.1, 0.25, 1.0}` is the
    /// CSS `ease`.)
    CubicBezier { x1: f32, y1: f32, x2: f32, y2: f32 },
    /// Arbitrary curve (should map `0 → ~0`, `1 → ~1`) — the no-`dyn` escape.
    Custom(fn(f32) -> f32),
}

impl Interpolator {
    /// Ease `t` (clamped to `[0, 1]`) along this curve. The result may leave
    /// `[0, 1]` only for a `Custom`/overshooting curve.
    pub fn interpolate(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Interpolator::Linear => t,
            Interpolator::EaseIn => t * t * t,
            Interpolator::EaseOut => ease_out(t),
            Interpolator::EaseInOut => ease_in_out(t),
            Interpolator::CubicBezier { x1, y1, x2, y2 } => cubic_bezier(t, x1, y1, x2, y2),
            Interpolator::Custom(f) => f(t),
        }
    }
}

/// Evaluate a CSS-style cubic Bézier easing at `t`: solve `x(s) = t` for the
/// curve parameter `s` (Newton–Raphson, clamped), then return `y(s)`. Control
/// points are `(0,0)`, `(x1,y1)`, `(x2,y2)`, `(1,1)`.
fn cubic_bezier(t: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    fn axis(p1: f32, p2: f32, s: f32) -> f32 {
        let u = 1.0 - s;
        3.0 * u * u * s * p1 + 3.0 * u * s * s * p2 + s * s * s
    }
    fn axis_deriv(p1: f32, p2: f32, s: f32) -> f32 {
        let u = 1.0 - s;
        3.0 * u * u * p1 + 6.0 * u * s * (p2 - p1) + 3.0 * s * s * (1.0 - p2)
    }
    let mut s = t;
    for _ in 0..8 {
        let dx = axis(x1, x2, s) - t;
        if dx.abs() < 1e-5 {
            break;
        }
        let d = axis_deriv(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - dx / d).clamp(0.0, 1.0);
    }
    axis(y1, y2, s)
}

/// A value that can be interpolated between two endpoints by an eased factor —
/// the generic `Animation<T>` operates on any `Animatable`.
pub trait Animatable: Copy + PartialEq {
    /// Linear blend `from → to` by `t` (the already-eased progress).
    fn lerp(from: Self, to: Self, t: f32) -> Self;
}

impl Animatable for f32 {
    fn lerp(from: f32, to: f32, t: f32) -> f32 {
        from + (to - from) * t
    }
}

impl Animatable for Vec2 {
    fn lerp(from: Vec2, to: Vec2, t: f32) -> Vec2 {
        from + (to - from) * t
    }
}

/// RGBA (linear) — per-channel blend, for colour fades / highlight transitions.
impl Animatable for [f32; 4] {
    fn lerp(from: [f32; 4], to: [f32; 4], t: f32) -> [f32; 4] {
        [
            from[0] + (to[0] - from[0]) * t,
            from[1] + (to[1] - from[1]) * t,
            from[2] + (to[2] - from[2]) * t,
            from[3] + (to[3] - from[3]) * t,
        ]
    }
}

/// A time-based tween of an [`Animatable`] value — the single animation object.
///
/// It stores the EPOCH (`from`/`to`/`start`/`duration`/`interp`), never a
/// resolved value (R12): the coordinator holds one of these (bucket 3-S) and
/// reads [`value`](Animation::value) each frame against the frame clock.
/// [`retarget`](Animation::retarget) re-aims at a new target while rebasing
/// `from` to the CURRENT value, so a reversal never jumps; [`snap`](
/// Animation::snap) sets the value instantly (a drag). One object replaces the
/// hand-written epoch/step/derive triple per animation.
#[derive(Debug, Clone, Copy)]
pub struct Animation<T: Animatable> {
    from: T,
    to: T,
    start: Instant,
    duration: Duration,
    interp: Interpolator,
}

impl<T: Animatable> Animation<T> {
    /// A settled (non-animating) animation resting at `value`. `now` seeds the
    /// epoch; since `from == to` it reports `value` and `animating() == false`
    /// regardless of the clock.
    pub fn settled(value: T, now: Instant, duration: Duration, interp: Interpolator) -> Self {
        Self { from: value, to: value, start: now, duration, interp }
    }

    /// Re-aim at `to`, rebasing `from` to the current value so a mid-flight
    /// reversal animates from where it is (no jump). No-op when already heading
    /// to `to` — safe to call every frame with the same target.
    pub fn retarget(&mut self, to: T, now: Instant) {
        if self.to == to {
            return;
        }
        self.from = self.value(now);
        self.to = to;
        self.start = now;
    }

    /// Set the value instantly, cancelling any in-flight tween (a hand-drag).
    pub fn snap(&mut self, value: T) {
        self.from = value;
        self.to = value;
    }

    /// The eased value at `now`: `lerp(from, to, interp(progress))`, where
    /// `progress = (now - start) / duration` clamped to `[0, 1]`.
    pub fn value(&self, now: Instant) -> T {
        if self.from == self.to || self.duration.is_zero() {
            return self.to;
        }
        let t = (now.saturating_duration_since(self.start).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        T::lerp(self.from, self.to, self.interp.interpolate(t))
    }

    /// Whether a tween is still in flight (`from != to` and `t < 1`). Drives the
    /// caller's "request another frame" decision.
    pub fn animating(&self, now: Instant) -> bool {
        self.from != self.to && now < self.start + self.duration
    }

    /// The value the tween is heading toward.
    pub fn target(&self) -> T {
        self.to
    }
}

/// Linear ramp; `t` clamped to `[0, 1]`.
pub fn linear(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}

/// Ease-out cubic (decelerating); `t` clamped to `[0, 1]`. `ease_out(0) == 0`,
/// `ease_out(1) == 1`.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Symmetric ease-in-out cubic; `t` clamped to `[0, 1]`. `ease_in_out(0) == 0`,
/// `ease_in_out(0.5) == 0.5`, `ease_in_out(1) == 1`.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let f = -2.0 * t + 2.0;
        1.0 - f * f * f / 2.0
    }
}

/// Linear interpolation from `from` to `to` by `d` (unclamped — `d` is usually
/// an already-eased `[0, 1]` progress).
pub fn lerp(from: f32, to: f32, d: f32) -> f32 {
    from + (to - from) * d
}

/// Multiply `alpha` (clamped to `[0, 1]`) into the alpha channel of every rect,
/// glyph, and shadow in `out` (RGB untouched), baking a global opacity for the
/// overlay layer. Run on the popup's freshly-painted scratch output BEFORE it
/// is merged into the overlay, so ONLY the popup fades — the chrome beneath
/// keeps full opacity.
pub fn apply_overlay_alpha(out: &mut PaintOutput, alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    for r in &mut out.rects {
        r.color[3] *= a;
    }
    for g in &mut out.glyphs {
        g.color[3] *= a;
    }
    for s in &mut out.shadows {
        s.color[3] *= a;
    }
}
