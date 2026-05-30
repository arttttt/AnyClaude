//! Pixel-based scroll state and velocity tracking.
//!
//! See `docs/design/gpu-terminal-scroll.md` for the full design. The momentum
//! integrator that consumes `ScrollVelocity` is added in a later commit.
//!
//! Constants below are copied verbatim from Warp (MIT), see
//! `docs/analysis/warp-rendering-research.md` §6.4.

use std::time::Instant;

use glam::Vec2;

/// Wheel-mouse LineDelta multiplier. Matches Warp / Chromium / Flutter.
pub const NUM_PIXELS_PER_LINE: f32 = 40.0;

/// Reference interval for the momentum decay factor. Also used as the default
/// time_delta when sampling velocity for the first time.
pub const MOMENTUM_DECAY_INTERVAL: f32 = 0.008;

/// Floor on time_delta when computing instantaneous velocity. Without this,
/// batched winit events (5+ wheel deltas delivered in one cycle) make
/// time_delta near-zero and velocity explodes.
pub const MIN_VELOCITY_TIME_DELTA: f32 = 0.004;

/// Multiplicative decay applied once per `MOMENTUM_DECAY_INTERVAL`. Over one
/// second of momentum, velocity multiplies by ~0.018 (near-stop in ~1.5 s).
pub const MOMENTUM_DECAY: f32 = 0.968;

/// Momentum tick frequency: 8 ms = ~125 Hz, decoupled from display vsync.
pub const MOMENTUM_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(8);

/// Minimum velocity (px/s) at swipe-end required to kick off momentum.
pub const MOMENTUM_THRESHOLD: f32 = 50.0;

/// Below this velocity (px/s) the momentum loop stops.
pub const MOMENTUM_MIN_VELOCITY: f32 = 1.0;

/// Clamp on initial momentum velocity (px/s). Defends against batched-event
/// spikes that survive `MIN_VELOCITY_TIME_DELTA`.
pub const MOMENTUM_MAX_VELOCITY: f32 = 2000.0;

/// Time after the last wheel event before we consider a gesture "ended" and
/// inertia can kick in. winit does not deliver a Phase::Ended event for
/// trackpads on macOS, so we infer end-of-gesture by silence.
pub const GESTURE_END_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(50);

/// Pixel-precise scroll position. Replaces the line-based `scrollback_offset`
/// that produced tmux-style stepping.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScrollState {
    /// Pixel offset from the top of the scrollable content. 0.0 = top.
    pub offset_y: f32,
    /// Total scrollable content height in pixels.
    pub total_size_px: f32,
    /// Visible viewport height in pixels.
    pub visible_px: f32,
}

impl ScrollState {
    pub fn max_offset(&self) -> f32 {
        (self.total_size_px - self.visible_px).max(0.0)
    }

    /// Apply a delta in pixels and clamp into `[0, max_offset]`.
    pub fn scroll_by(&mut self, dy_px: f32) {
        self.offset_y = (self.offset_y + dy_px).clamp(0.0, self.max_offset());
    }
}

/// Instantaneous scroll velocity in pixels per second.
#[derive(Debug, Clone, Copy)]
pub struct ScrollVelocity {
    pub velocity: Vec2,
    pub last_update: Instant,
}

impl ScrollVelocity {
    /// Compute a new velocity sample from a position delta and the previous
    /// sample. `time_delta` is floored at `MIN_VELOCITY_TIME_DELTA` to keep
    /// batched event spikes from inflating velocity.
    pub fn record(prev: Option<Self>, delta: Vec2, now: Instant) -> Self {
        let time_delta = prev
            .map(|v| now.duration_since(v.last_update).as_secs_f32())
            .unwrap_or(MOMENTUM_DECAY_INTERVAL)
            .max(MIN_VELOCITY_TIME_DELTA);
        Self {
            velocity: delta / time_delta,
            last_update: now,
        }
    }

    /// Clamp velocity magnitude to `MOMENTUM_MAX_VELOCITY`. Returns the
    /// possibly-rescaled velocity vector.
    pub fn clamped_for_momentum(self) -> Vec2 {
        let speed = self.velocity.length();
        if speed > MOMENTUM_MAX_VELOCITY {
            self.velocity * (MOMENTUM_MAX_VELOCITY / speed)
        } else {
            self.velocity
        }
    }
}

/// Apply exponential decay to `velocity` over `elapsed` seconds, at the
/// reference cadence `MOMENTUM_DECAY_INTERVAL` with factor `MOMENTUM_DECAY`.
pub fn decay_velocity(velocity: Vec2, elapsed: f32) -> Vec2 {
    velocity * MOMENTUM_DECAY.powf(elapsed / MOMENTUM_DECAY_INTERVAL)
}
