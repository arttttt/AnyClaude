//! Popup open/close fade transition (R12): a stored epoch + a pure step/derive
//! pair, kept out of the coordinator's `redraw` so the logic is headlessly
//! testable. The coordinator holds one `Option<PopupAnim>` (bucket 3-S) and,
//! each frame, calls [`step_popup_anim`] to advance the epoch on a visibility
//! EDGE and [`popup_fade_alpha`] to derive the frame's alpha — the alpha itself
//! is never stored.

use std::time::Instant;

use term_ui::ease_out;

/// Open/close fade epoch: when the current transition began + its direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopupAnim {
    pub started_at: Instant,
    pub opening: bool,
}

/// Advance the fade epoch for this frame from the previous epoch, whether a
/// popup is currently visible, and the frame clock. The epoch resets (to `now`)
/// only on a visibility EDGE — open (`None` → visible), close (opening →
/// hidden), or a mid-fade reversal (closing → visible); a steady state holds
/// the prior epoch unchanged.
///
/// A reversal restarts the new direction at `t = 0`, so a popup reversed
/// mid-fade snaps to that direction's start alpha and animates from there.
/// Given the 0.12s fade this is imperceptible in practice and is intentionally
/// left un-smoothed (no inverse-easing carry-over).
pub fn step_popup_anim(prev: Option<PopupAnim>, visible: bool, now: Instant) -> Option<PopupAnim> {
    match prev {
        None if visible => Some(PopupAnim { started_at: now, opening: true }),
        Some(a) if a.opening && !visible => Some(PopupAnim { started_at: now, opening: false }),
        Some(a) if !a.opening && visible => Some(PopupAnim { started_at: now, opening: true }),
        other => other,
    }
}

/// Derive `(alpha, animating)` for the frame: the eased opacity — opening ramps
/// `0 → 1`, closing `1 → 0` — and whether the fade is still in flight (`t < 1`).
/// `None` (no transition) is fully opaque and not animating.
pub fn popup_fade_alpha(anim: Option<PopupAnim>, now: Instant, fade_secs: f32) -> (f32, bool) {
    match anim {
        Some(a) => {
            let t = (now.saturating_duration_since(a.started_at).as_secs_f32() / fade_secs).min(1.0);
            let alpha = if a.opening { ease_out(t) } else { 1.0 - ease_out(t) };
            (alpha, t < 1.0)
        }
        None => (1.0, false),
    }
}
