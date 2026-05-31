//! Panel overlay collapse/expand transition (R12): a stored epoch + a pure
//! step/derive pair, mirroring [`crate::ui::popup_anim`] but driving a WIDTH
//! factor instead of an opacity alpha. The coordinator holds one
//! `Option<PanelAnim>` (bucket 3-S) and, each frame, calls [`step_panel_anim`]
//! to advance the epoch on a visibility EDGE and [`panel_width_factor`] to
//! derive the frame's `0..=1` factor (collapsed → expanded). The factor itself
//! is never stored; the rendered width is `lerp(strip_w, target_w, factor)`.

use std::time::Instant;

use term_ui::ease_in_out;

/// Collapse/expand epoch: when the current transition began + its direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelAnim {
    pub started_at: Instant,
    pub expanding: bool,
}

/// Advance the epoch for this frame from the previous epoch, whether the manager
/// is currently visible (expanded), and the frame clock. The epoch resets (to
/// `now`) only on a visibility EDGE — expand (`None`/collapsing → visible),
/// collapse (expanding → hidden), or a mid-animation reversal; a steady state
/// holds the prior epoch. A reversal restarts the new direction at `t = 0`
/// (imperceptible at the sub-200 ms duration; left un-smoothed like the popup).
pub fn step_panel_anim(prev: Option<PanelAnim>, visible: bool, now: Instant) -> Option<PanelAnim> {
    match prev {
        None if visible => Some(PanelAnim { started_at: now, expanding: true }),
        Some(a) if a.expanding && !visible => Some(PanelAnim { started_at: now, expanding: false }),
        Some(a) if !a.expanding && visible => Some(PanelAnim { started_at: now, expanding: true }),
        other => other,
    }
}

/// Derive `(factor, animating)` for the frame: the eased width factor — expanding
/// ramps `0 → 1`, collapsing `1 → 0` — and whether the transition is still in
/// flight (`t < 1`). With no epoch the factor is the steady state: `1.0` when
/// visible (fully expanded), `0.0` when hidden (fully collapsed).
pub fn panel_width_factor(
    anim: Option<PanelAnim>,
    visible: bool,
    now: Instant,
    anim_secs: f32,
) -> (f32, bool) {
    match anim {
        Some(a) => {
            let t = (now.saturating_duration_since(a.started_at).as_secs_f32() / anim_secs).min(1.0);
            let factor = if a.expanding { ease_in_out(t) } else { 1.0 - ease_in_out(t) };
            (factor, t < 1.0)
        }
        None => (if visible { 1.0 } else { 0.0 }, false),
    }
}
