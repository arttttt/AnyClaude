//! Pure terminal-area geometry + hit-testing — no `&self`, no GPU, no PTY.
//!
//! These are the decision functions the GPU UI mixes into `&mut self` methods
//! today (`GpuApp::{terminal_panel_rect, fit_grid, cell_at, bump_click_count}`).
//! Pulled out as free, unit-tested functions: the live `GpuApp` delegates to
//! them now, and the term_ui coordinator that replaces it (Phase E) reuses the
//! same logic in `apply`/`view`. Cell sizes are passed as plain physical-pixel
//! `f32`s (not `CellMetrics`) so the math is testable without a GPU.

use std::time::Instant;

use term_gpu::{CellPoint, PanelRect};

/// A recorded click, for multi-click (double / triple) detection.
#[derive(Debug, Clone, Copy)]
pub struct LastClick {
    pub time: Instant,
    pub point: CellPoint,
    pub count: u32,
}

/// The terminal panel rect (logical px) carved out between the top header
/// chrome (`header_h`) and the bottom footer chrome (`footer_h`), inset
/// horizontally by `h_pad` on each side. Width/height are clamped to ≥ 0 so a
/// tiny window never yields a negative extent. (The chrome bars and their
/// separators stay full-width; only the content is inset.)
pub fn terminal_panel_rect(
    w_logical: f32,
    h_logical: f32,
    header_h: f32,
    footer_h: f32,
    h_pad: f32,
) -> PanelRect {
    let h = (h_logical - header_h - footer_h).max(0.0);
    let w = (w_logical - 2.0 * h_pad).max(0.0);
    PanelRect::new(h_pad, header_h, w, h)
}

/// Cols × rows that fit inside `panel` at the given physical cell size, each
/// clamped to ≥ 1 (a sub-cell area is degenerate but must not yield 0).
pub fn fit_grid(panel: PanelRect, cell_w_physical: f32, cell_h_physical: f32, sf: f32) -> (usize, usize) {
    let sf = sf.max(0.0001);
    let cols = ((panel.w * sf / cell_w_physical).floor() as usize).max(1);
    let rows = ((panel.h * sf / cell_h_physical).floor() as usize).max(1);
    (cols, rows)
}

/// The cell under a window-local logical-pixel `(x, y)`, or `None` when the
/// cell size is degenerate. Inverse of `populate_panel`'s row positioning:
///   row_y = row_idx * cell_h - baseline_offset + scroll_offset
/// where `baseline_offset` accounts for scrollback above the visible window.
/// `total_rows`/`visible_rows`/`cols` come from the emulator snapshot.
#[allow(clippy::too_many_arguments)]
pub fn cell_at(
    x: f32,
    y: f32,
    panel: PanelRect,
    cell_w_physical: f32,
    cell_h_physical: f32,
    sf: f32,
    scroll_offset_y: f32,
    total_rows: usize,
    visible_rows: usize,
    cols: usize,
) -> Option<CellPoint> {
    let sf = sf.max(0.0001);
    let cell_w_logical = cell_w_physical / sf;
    let cell_h_logical = cell_h_physical / sf;
    if cell_w_logical <= 0.0 || cell_h_logical <= 0.0 {
        return None;
    }
    // Mouse coords are window-relative; translate into the terminal area so the
    // row math matches `populate_panel`.
    let local_x = (x - panel.x).max(0.0);
    let local_y = (y - panel.y).max(0.0);
    let baseline_offset = total_rows.saturating_sub(visible_rows) as f32 * cell_h_logical;
    let row_unclamped = ((local_y + baseline_offset - scroll_offset_y) / cell_h_logical).floor();
    let row = row_unclamped.clamp(0.0, total_rows.saturating_sub(1) as f32) as usize;
    let col_unclamped = (local_x / cell_w_logical).floor();
    let col = col_unclamped.clamp(0.0, cols.saturating_sub(1) as f32) as usize;
    Some(CellPoint { row, col })
}

/// The multi-click count (1..=3) for a new click at `point`, given the prior
/// click. Increments while the click stays on the same cell within
/// `threshold_ms`, wraps 3 → 1, and resets to 1 on a different cell or after
/// the threshold.
pub fn next_click_count(
    last: Option<LastClick>,
    point: CellPoint,
    now: Instant,
    threshold_ms: u128,
) -> u32 {
    match last {
        Some(lc)
            if lc.point == point && now.duration_since(lc.time).as_millis() <= threshold_ms =>
        {
            if lc.count >= 3 {
                1
            } else {
                lc.count + 1
            }
        }
        _ => 1,
    }
}
