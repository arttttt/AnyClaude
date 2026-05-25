//! Pixel-based scroll state.
//!
//! See `docs/design/gpu-terminal-scroll.md` for the full design, including
//! velocity tracking and the momentum integrator that later commits add.

/// Wheel-mouse LineDelta multiplier. Matches Warp / Chromium / Flutter.
pub const NUM_PIXELS_PER_LINE: f32 = 40.0;

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
