//! Geometry + mouse hit-testing: cell metrics, the terminal panel rect, grid
//! fit, scroll-bounds refresh, pixel→cell translation, and the mouse-press /
//! mouse-report encoding paths.

use std::time::Instant;

use glam::Vec2;
use term_gpu::{
    encode_motion_report, encode_mouse_report, measure_cell_metrics, CellMetrics, CellPoint,
    MouseButton, MouseEventKind, PanelRect,
};

use winit::window::CursorIcon;

use crate::ui::app_state::{ApplyCtx, Msg};
use crate::ui::gpu::chrome::{CHROME_H_PAD, FOOTER_HEIGHT_LOGICAL, HEADER_HEIGHT_LOGICAL};
use crate::ui::panel_manager::ManagerId;
use crate::ui::term_geometry;

use super::{FONT_SIZE, MULTI_CLICK_THRESHOLD_MS};

impl super::GpuApp {
    pub(super) fn cell_metrics(&mut self) -> CellMetrics {
        if let Some(m) = self.text.cell_metrics {
            return m;
        }
        let metrics = measure_cell_metrics(
            &mut self.text.font_system,
            &mut self.text.shape_cache,
            FONT_SIZE,
            self.scale_factor,
        );
        self.text.cell_metrics = Some(metrics);
        metrics
    }

    /// The terminal area sits below the header chrome. Returns the
    /// rect (logical pixels, top-left origin) callers should pass to
    /// `populate_panel` / `build_cursor_rect` and use as the basis
    /// for mouse hit-testing.
    pub(super) fn terminal_panel_rect(&self) -> PanelRect {
        let Some(window) = self.window.as_ref() else {
            return PanelRect::new(0.0, HEADER_HEIGHT_LOGICAL, 0.0, 0.0);
        };
        let size = window.inner_size();
        let sf = self.scale_factor.max(0.0001);
        let w_logical = size.width as f32 / sf;
        let h_logical = size.height as f32 / sf;
        term_geometry::terminal_panel_rect(
            w_logical,
            h_logical,
            HEADER_HEIGHT_LOGICAL,
            FOOTER_HEIGHT_LOGICAL,
            CHROME_H_PAD,
        )
    }

    /// Compute the grid size (cols × rows) that fits inside the
    /// terminal panel rect at the current cell metrics. Both
    /// dimensions are clamped to at least 1 — a sub-cell terminal
    /// area is degenerate but should never panic.
    pub(super) fn fit_grid(&mut self) -> (usize, usize) {
        let metrics = self.cell_metrics();
        let panel = self.terminal_panel_rect();
        term_geometry::fit_grid(
            panel,
            metrics.width_physical,
            metrics.height_physical,
            self.scale_factor,
        )
    }

    /// Resync emulator + PTY to the current window size. Called from
    /// `resumed` and on `Resized`/`ScaleFactorChanged`.
    pub(super) fn resync_grid(&mut self) {
        let (cols, rows) = self.fit_grid();
        self.dispatch(Msg::GridResized { cols, rows });
    }

    /// Recompute the scroll bounds from the current emulator snapshot
    /// and window size. Called before any scroll mutation so clamping
    /// uses up-to-date geometry.
    pub(super) fn refresh_scroll_geometry(&mut self) {
        let metrics = self.cell_metrics();
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Some(emu) = self.session.emulator.as_ref() else {
            return;
        };
        let sf = self.scale_factor.max(0.0001);
        let cell_h_logical = metrics.height_physical / sf;
        let snap = emu.snapshot();
        let visible_h_logical = window.inner_size().height as f32 / sf;
        self.state.scroll.total_size_px = snap.rows.len() as f32 * cell_h_logical;
        self.state.scroll.visible_px = visible_h_logical;
        let max = self.state.scroll.max_offset();
        if self.state.scroll.offset_y > max {
            self.state.scroll.offset_y = max;
        }
    }

    /// Translate a window-local logical-pixel position into the cell
    /// underneath. Inverse of `populate_panel`'s row positioning:
    ///   row_y_logical = row_idx * cell_h - baseline_offset + scroll_offset
    ///   row_idx       = (row_y_logical + baseline_offset - scroll_offset) / cell_h
    pub(super) fn cell_at(&mut self, x: f32, y: f32) -> Option<CellPoint> {
        let metrics = self.cell_metrics();
        let panel = self.terminal_panel_rect();
        let emu = self.session.emulator.as_ref()?;
        let snap = emu.snapshot();
        let total_rows = snap.rows.len();
        let visible_rows = snap.visible_rows;
        let cols = snap.rows.first().map(|r| r.cells.len()).unwrap_or(0);
        term_geometry::cell_at(
            x,
            y,
            panel,
            metrics.width_physical,
            metrics.height_physical,
            self.scale_factor,
            self.state.scroll.offset_y,
            total_rows,
            visible_rows,
            cols,
        )
    }

    /// Translate a left-press into `Msg::MousePress` and run it. The coordinator
    /// pre-resolves the resource-backed gates — header band, session-id hot-zone,
    /// mouse-reporting mode, and the cell under the cursor — and hands the
    /// emulator snapshot in the ctx so `apply` can word/line-expand a
    /// multi-click selection. The press decision itself lives in `apply`.
    /// The cursor icon to show for the mouse at `(x, y)`: a horizontal-resize
    /// cursor over a resizable panel's inner edge (or while dragging it), a
    /// pointer over the toggle pill, else the default. Reuses the materialized
    /// overlay hit-zones so it costs no extra layout.
    pub(super) fn panel_hover_cursor(&self, x: f32, y: f32) -> CursorIcon {
        if self.state.panel_edge_drag.is_some() {
            return CursorIcon::EwResize;
        }
        let p = Vec2::new(x, y);
        // The pill straddles the divider (partly outside the overlay rect), so
        // check it first.
        if self.panel_toggle_zone.is_some_and(|b| b.contains(p)) {
            return CursorIcon::Pointer;
        }
        let Some(rect) = self.panel_overlay_rect else {
            return CursorIcon::Default;
        };
        if !rect.contains(p) {
            return CursorIcon::Default;
        }
        if self.state.right.policy().resizable
            && x <= rect.origin.x + self.state.right.policy().collapsed_width
        {
            return CursorIcon::EwResize;
        }
        CursorIcon::Default
    }

    /// Set the window cursor for a hover at `(x, y)`, only calling `set_cursor`
    /// when the icon actually changes (cached in `current_cursor`).
    pub(super) fn update_hover_cursor(&mut self, x: f32, y: f32) {
        let desired = self.panel_hover_cursor(x, y);
        if desired != self.current_cursor {
            if let Some(w) = self.window.as_ref() {
                w.set_cursor(desired);
            }
            self.current_cursor = desired;
        }
    }

    pub(super) fn on_mouse_press(&mut self) {
        let Some((x, y)) = self.state.cursor_pos else { return };
        let p = Vec2::new(x, y);
        // The toggle pill straddles the divider (partly outside the overlay
        // rect), so a click on it collapses/expands FIRST, independent of the
        // overlay rect.
        if self.panel_toggle_zone.is_some_and(|b| b.contains(p)) {
            self.dispatch(Msg::PanelToggle(ManagerId::Right));
            return;
        }
        // The right overlay floats over the terminal, so it takes the press
        // next: an inner-edge click begins a width drag (available collapsed OR
        // expanded), and every other in-overlay click is swallowed so it doesn't
        // start a terminal selection underneath.
        if let Some(rect) = self.panel_overlay_rect {
            if rect.contains(p) {
                let on_edge = x <= rect.origin.x + self.state.right.policy().collapsed_width;
                if self.state.right.policy().resizable && on_edge {
                    self.dispatch(Msg::PanelEdgeDragStart(ManagerId::Right));
                }
                return;
            }
        }
        let in_header = y < HEADER_HEIGHT_LOGICAL;
        let in_session_zone = self
            .session_click_zone
            .map(|(sx, ex)| x >= sx && x < ex)
            .unwrap_or(false);
        let point = self.cell_at(x, y);
        // When an app has mouse reporting on, the press is encoded for the PTY
        // (and apply suppresses selection) — §6.
        let mouse_report = point.and_then(|p| {
            self.mouse_report(
                MouseButton::Left,
                MouseEventKind::Press,
                p.col as u16 + 1,
                p.row as u16 + 1,
            )
        });
        let snapshot = self.session.emulator.as_ref().map(|e| e.snapshot());
        let ctx = ApplyCtx {
            now: Instant::now(),
            snapshot: snapshot.as_ref(),
            multi_click_threshold_ms: MULTI_CLICK_THRESHOLD_MS,
        };
        let fx = self.state.apply(
            Msg::MousePress { in_header, in_session_zone, point, mouse_report },
            &ctx,
        );
        let _ = self.perform_effects(fx);
    }

    /// Encode a mouse event for the PTY when an app has reporting on (§6), or
    /// `None` when reporting is off, the active tracking level doesn't report
    /// this event (motion is only 1002 / 1003), or Shift is held (the bypass
    /// that keeps the gesture a local selection / scroll — matching Warp).
    /// `col` / `row` are 1-based cells (the snapshot cell + 1 — viewport-correct
    /// on the alt screen, where mouse-mode apps live and there's no scrollback).
    /// The tracking level + encoding come from the emulator's split
    /// `MouseProtocol`; the byte shape is decided in `encode_mouse_report`.
    fn mouse_report(
        &self,
        button: MouseButton,
        kind: MouseEventKind,
        col: u16,
        row: u16,
    ) -> Option<Vec<u8>> {
        // Shift is the local-action bypass: forward nothing so the gesture
        // stays a selection / scroll even while an app has tracking on.
        if self.state.modifiers.shift_key() {
            return None;
        }
        let proto = self.session.emulator.as_ref()?.mouse_protocol();
        if !proto.is_active() {
            return None;
        }
        if matches!(kind, MouseEventKind::Motion) && !proto.reports_motion() {
            return None;
        }
        Some(encode_mouse_report(button, kind, col, row, proto.is_sgr()))
    }

    /// The mouse report for the cell currently under the cursor (release /
    /// wheel / motion), or `None` when reporting is off / the cursor isn't over
    /// a cell.
    pub(super) fn mouse_report_at_cursor(
        &mut self,
        button: MouseButton,
        kind: MouseEventKind,
    ) -> Option<Vec<u8>> {
        let (x, y) = self.state.cursor_pos?;
        let p = self.cell_at(x, y)?;
        self.mouse_report(button, kind, p.col as u16 + 1, p.row as u16 + 1)
    }

    /// Build a motion (drag / move) report for the already-resolved cell when a
    /// mouse-tracking app wants motion. Applies the Shift bypass (keeps the drag
    /// a local selection) and the per-cell dedup, then delegates the tracking /
    /// encoding decision to the pure `encode_motion_report`.
    pub(super) fn motion_report(&self, point: Option<CellPoint>) -> Option<Vec<u8>> {
        if self.state.modifiers.shift_key() {
            return None;
        }
        let proto = self.session.emulator.as_ref()?.mouse_protocol();
        let p = point?;
        encode_motion_report(
            proto,
            self.state.mouse_left_held,
            self.state.mouse_motion_cell,
            (p.col as u16, p.row as u16),
        )
    }
}
