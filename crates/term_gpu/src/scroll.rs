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
}
