//! Animation helpers (design §9/§12): pure easing curves + an overlay-alpha
//! bake. Transitions store NO resolved value (R12) — the coordinator derives a
//! transition's progress `t` from an epoch + the frame clock each frame, eases
//! it, and bakes the resulting alpha into a freshly-painted overlay
//! [`PaintOutput`] BEFORE merging it into the overlay layer. The bake is
//! non-incremental on purpose (§9): the whole overlay is re-emitted each
//! animating frame. Scoped to the emoji-free chrome/popup overlay — the
//! colour-glyph path ignores per-instance colour alpha.

use crate::paint::PaintOutput;

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
