//! Pure terminal-area geometry + hit-testing (Phase E.1). These are the
//! decision functions GpuApp now delegates to; pinning them here lets the
//! coordinator reuse them with confidence (and the live app exercises the same
//! code, so a regression shows up both in these tests and on screen).

use std::time::{Duration, Instant};

use anyclaude::ui::term_geometry::{cell_at, fit_grid, next_click_count, terminal_panel_rect, LastClick};
use term_gpu::{CellPoint, PanelRect};

#[test]
fn panel_sits_between_header_and_footer() {
    let p = terminal_panel_rect(800.0, 600.0, 24.0, 22.0);
    assert_eq!(p.x, 0.0);
    assert_eq!(p.y, 24.0);
    assert_eq!(p.w, 800.0);
    assert_eq!(p.h, 600.0 - 24.0 - 22.0);
}

#[test]
fn panel_height_clamps_to_zero_when_window_smaller_than_chrome() {
    let p = terminal_panel_rect(800.0, 30.0, 24.0, 22.0);
    assert_eq!(p.h, 0.0);
}

#[test]
fn fit_grid_floors_and_clamps_to_at_least_one() {
    let panel = PanelRect::new(0.0, 24.0, 800.0, 554.0);
    // 800/8 = 100 cols, 554/16 = 34.6 → 34 rows.
    assert_eq!(fit_grid(panel, 8.0, 16.0, 1.0), (100, 34));

    // A sub-cell panel still yields at least 1×1.
    let tiny = PanelRect::new(0.0, 24.0, 2.0, 2.0);
    assert_eq!(fit_grid(tiny, 8.0, 16.0, 1.0), (1, 1));
}

#[test]
fn fit_grid_accounts_for_scale_factor() {
    // panel.w logical = 400, sf = 2 → 800 physical / 16 = 50 cols.
    let panel = PanelRect::new(0.0, 24.0, 400.0, 320.0);
    assert_eq!(fit_grid(panel, 16.0, 32.0, 2.0), (50, 20));
}

#[test]
fn cell_at_maps_local_pixels_to_row_col() {
    let panel = PanelRect::new(0.0, 24.0, 800.0, 554.0);
    // No scrollback (total == visible) → baseline_offset 0.
    // y = 24 (panel top) + 40 → local_y 40 → row 40/16 = 2; x 20 → col 20/8 = 2.
    let hit = cell_at(20.0, 64.0, panel, 8.0, 16.0, 1.0, 0.0, 30, 30, 100);
    assert_eq!(hit, Some(CellPoint { row: 2, col: 2 }));
}

#[test]
fn cell_at_offsets_by_scrollback_baseline() {
    let panel = PanelRect::new(0.0, 24.0, 800.0, 554.0);
    // 50 total rows, 30 visible → baseline_offset = 20 * 16 = 320 logical.
    // Click at the very top of the panel maps to the first VISIBLE row (20).
    let hit = cell_at(0.0, 24.0, panel, 8.0, 16.0, 1.0, 0.0, 50, 30, 100);
    assert_eq!(hit, Some(CellPoint { row: 20, col: 0 }));
}

#[test]
fn cell_at_clamps_into_grid_bounds() {
    let panel = PanelRect::new(0.0, 24.0, 800.0, 554.0);
    // Click far below the last row clamps to total_rows - 1; far right to cols-1.
    let hit = cell_at(10_000.0, 10_000.0, panel, 8.0, 16.0, 1.0, 0.0, 30, 30, 100);
    assert_eq!(hit, Some(CellPoint { row: 29, col: 99 }));
}

#[test]
fn cell_at_returns_none_on_degenerate_cell() {
    let panel = PanelRect::new(0.0, 24.0, 800.0, 554.0);
    assert_eq!(cell_at(20.0, 64.0, panel, 8.0, 0.0, 1.0, 0.0, 30, 30, 100), None);
}

#[test]
fn click_count_cycles_one_two_three_one() {
    let now = Instant::now();
    let p = CellPoint { row: 1, col: 1 };

    assert_eq!(next_click_count(None, p, now, 400), 1);

    let mut last = LastClick { time: now, point: p, count: 1 };
    let mut t = now;
    for expected in [2, 3, 1] {
        t += Duration::from_millis(100);
        let c = next_click_count(Some(last), p, t, 400);
        assert_eq!(c, expected);
        last = LastClick { time: t, point: p, count: c };
    }
}

#[test]
fn click_count_resets_on_different_cell() {
    let now = Instant::now();
    let p = CellPoint { row: 1, col: 1 };
    let other = CellPoint { row: 2, col: 2 };
    let last = LastClick { time: now, point: p, count: 1 };
    assert_eq!(next_click_count(Some(last), other, now + Duration::from_millis(50), 400), 1);
}

#[test]
fn click_count_resets_after_threshold() {
    let now = Instant::now();
    let p = CellPoint { row: 1, col: 1 };
    let last = LastClick { time: now, point: p, count: 1 };
    assert_eq!(next_click_count(Some(last), p, now + Duration::from_millis(500), 400), 1);
}
