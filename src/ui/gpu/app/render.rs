//! The per-frame paint pipeline: `GpuApp::redraw`.
//!
//! Builds the terminal base layer (grid + selection + cursor) directly via
//! `populate_panel` (R5: the grid stays immediate-emit, not retained), then
//! builds the chrome + popup term_ui views from the current `AppState` and hands
//! them to the [`OverlayRenderer`] for the retained-tree pipeline. The overlay
//! is drawn entirely after the terminal base, so the bars / popup sit on top.

use std::time::Instant;

use glam::Vec2;
use term_gpu::{
    build_cursor_rect, populate_panel, push_selection_rects, GlyphInstance, RectInstance,
    RenderLayer,
};
use term_ui::{Block, Bounds};

use crate::ui::chrome_labels;
use crate::ui::gpu::chrome::{
    CHROME_FONT_SIZE, CHROME_H_PAD, FOOTER_HEIGHT_LOGICAL, HEADER_HEIGHT_LOGICAL,
};
use crate::ui::panels_view;
use crate::ui::popup_view;

use super::FONT_SIZE;

impl super::GpuApp {
    /// Render one frame: clear, populate cells, push cursor, draw
    /// header chrome, present.
    pub(super) fn redraw(&mut self) {
        let metrics = self.cell_metrics();
        let panel = self.terminal_panel_rect();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Some(emulator) = self.session.emulator.as_ref() else {
            return;
        };
        let sf = self.scale_factor.max(0.0001);

        let snapshot = emulator.snapshot();
        let scroll_offset_y = self.state.scroll.offset_y;
        let mut rects: Vec<RectInstance> = Vec::new();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();
        populate_panel(
            &snapshot,
            panel,
            &self.text.palette,
            &mut self.text.font_system,
            &mut self.text.swash_cache,
            renderer.atlas_mut(),
            &mut self.text.shape_cache,
            FONT_SIZE,
            sf,
            metrics,
            scroll_offset_y,
            &mut rects,
            &mut glyphs,
        );
        if let Some(sel) = self.state.selection {
            push_selection_rects(
                &sel,
                &snapshot,
                panel,
                sf,
                metrics,
                scroll_offset_y,
                &mut rects,
            );
        }
        if let Some(cursor_rect) = build_cursor_rect(
            snapshot.cursor,
            snapshot.visible_start(),
            panel,
            sf,
            metrics,
            scroll_offset_y,
        ) {
            rects.push(cursor_rect);
        }

        // Chrome (header + footer) and any popup render in the OVERLAY layer,
        // which is drawn entirely AFTER the terminal base. So the bars' opaque
        // background covers any terminal glyph that scrolls into the bar band,
        // the bar text sits on top, and a popup sits on top of the bars.
        let mut overlay_shadows: Vec<term_gpu::ShadowInstance> = Vec::new();
        let mut overlay_rects: Vec<RectInstance> = Vec::new();
        // Round-rect overlay decorations (modifier backgrounds / borders) — the
        // chrome / popup / panels views emit them; drawn over the sharp rects and
        // under the glyphs.
        let mut overlay_round_rects: Vec<term_gpu::RoundRectInstance> = Vec::new();
        let mut overlay_glyphs: Vec<GlyphInstance> = Vec::new();

        // The copied-flash is DERIVED from the deadline + frame clock (R12) —
        // no stored boolean, no expiry mutation.
        let now = Instant::now();
        let active_backend = self.backends.backend_state.get_active_backend();
        let cfg = self.backends.backend_state.get_config();
        let resolve_display = |id: &str| -> Option<String> {
            cfg.backends
                .iter()
                .find(|b| b.name == id)
                .map(|b| b.display_name.clone())
        };
        let subagent_label = self
            .backends.subagent_backend
            .get()
            .and_then(|id| resolve_display(&id));
        let teammate_label = self
            .backends.teammate_backend
            .get()
            .and_then(|id| resolve_display(&id));
        let total_reqs: u64 = self
            .backends.observability
            .snapshot()
            .per_backend
            .values()
            .map(|m| m.total)
            .sum();
        let window_size = window.inner_size();
        let window_logical =
            Vec2::new(window_size.width as f32 / sf, window_size.height as f32 / sf);
        // Chrome (header + footer) is a term_ui view: build it from the current
        // AppState here (it needs the backend / observability data), then hand it
        // to the overlay renderer for the term_ui pipeline + the session hitbox.
        let header = chrome_labels::header_segments(
            &active_backend,
            subagent_label.as_deref(),
            teammate_label.as_deref(),
            total_reqs,
            self.state.uptime_secs(now),
            &self.state.session_id,
            self.state.session_copied(now),
        );
        let (footer_left, footer_right) = chrome_labels::footer_segments(env!("CARGO_PKG_VERSION"));
        let chrome = chrome_labels::chrome_view(
            &header,
            &footer_left,
            &footer_right,
            CHROME_FONT_SIZE,
            HEADER_HEIGHT_LOGICAL,
            FOOTER_HEIGHT_LOGICAL,
            CHROME_H_PAD,
        );
        self.session_click_zone = self.overlay.render_chrome(
            chrome,
            window_logical,
            &mut self.text.font_system,
            &mut self.text.swash_cache,
            renderer.atlas_mut(),
            &mut self.text.ui_shape_cache,
            sf,
            &mut overlay_rects,
            &mut overlay_round_rects,
            &mut overlay_glyphs,
        );
        // Right teammates overlay (M1: placeholder panels). Floats over the
        // terminal content on the right, drawn AFTER the chrome and BEFORE the
        // popup so a popup still sits on top. Empty + collapsed → `None`, which
        // tears the tree down and leaves the default app byte-identical. Spans
        // the content band (between header + footer), positioned at the right
        // edge; collapsed renders just the edge strip width.
        // Right overlay width via the `panel_width` tween (R12). A hand-drag
        // snaps it to the live cursor width (instant); otherwise it retargets the
        // collapse/expand slide between the bare strip width and the visible
        // target width. `now` is the frame clock used above.
        let strip_w = self.state.right.policy().collapsed_width;
        let right_visible = self.state.right.is_visible();
        let right_empty = self.state.right.is_empty();
        let dragging = self.state.right.drag_width();
        let overlay_w = if let Some(dw) = dragging {
            self.panel_width.snap(dw);
            dw
        } else {
            let target = if right_visible { self.state.right.width() } else { strip_w };
            self.panel_width.retarget(target, now);
            self.panel_width.value(now)
        };
        let panel_animating = self.panel_width.animating(now);
        // Show the panel stack whenever the overlay is wider than the bare strip.
        let expanded = overlay_w > strip_w + 1.0;
        let panels = if right_empty && !right_visible && !panel_animating && dragging.is_none() {
            None
        } else {
            Some(panels_view::panel_manager_view(&self.state.right, expanded))
        };
        let has_panels = panels.is_some();
        let overlay_origin =
            Vec2::new((window_logical.x - overlay_w).max(0.0), HEADER_HEIGHT_LOGICAL);
        let overlay_size = Vec2::new(
            overlay_w,
            (window_logical.y - HEADER_HEIGHT_LOGICAL - FOOTER_HEIGHT_LOGICAL).max(0.0),
        );
        self.panel_toggle_zone = self.overlay.render_panels(
            panels,
            overlay_origin,
            overlay_size,
            &mut self.text.font_system,
            &mut self.text.swash_cache,
            renderer.atlas_mut(),
            &mut self.text.ui_shape_cache,
            sf,
            &mut overlay_rects,
            &mut overlay_round_rects,
            &mut overlay_glyphs,
        );
        self.panel_overlay_rect = has_panels.then(|| Bounds::new(overlay_origin, overlay_size));

        // Popup overlay — all three popups render via the term_ui SECOND TREE.
        // The backend switch needs runtime data AppState doesn't carry (the
        // backend list + active/override ids), so it is built here via
        // popup_view::backend_view; history + settings come straight from
        // AppState via popup_view::popup_view. Whichever is open is reconciled
        // into the popup tree, measured with a min-width floor, centred with
        // place_centered, and painted into the overlay on top of the chrome (its
        // term_ui Block drop shadow flows through too). Popups are mutually
        // exclusive, so at most one is ever built.
        let popup: Option<Block> = if self.state.backend_switch.is_visible() {
            let items_and_ids: Vec<(String, String)> = self
                .backends.backend_state
                .get_config()
                .backends
                .iter()
                .map(|b| (b.display_name.clone(), b.name.clone()))
                .collect();
            let active_backend = self.backends.backend_state.get_active_backend();
            let current_subagent = self.backends.subagent_backend.get();
            let current_teammate = self.backends.teammate_backend.get();
            Some(popup_view::backend_view(
                &self.state.backend_switch,
                &items_and_ids,
                &active_backend,
                current_subagent.as_deref(),
                current_teammate.as_deref(),
            ))
        } else {
            popup_view::popup_view(&self.state)
        };
        // Reconcile + lay out + paint the popup (fade-aware) on top of the
        // chrome; `popup_animating` keeps the redraw loop alive until a fade ends.
        let popup_animating = self.overlay.render_popup(
            popup,
            window_logical,
            now,
            popup_view::POPUP_MIN_WIDTH,
            &mut self.text.font_system,
            &mut self.text.swash_cache,
            renderer.atlas_mut(),
            &mut self.text.ui_shape_cache,
            sf,
            &mut overlay_shadows,
            &mut overlay_rects,
            &mut overlay_round_rects,
            &mut overlay_glyphs,
        );
        // The overlay always carries the chrome bars (and a popup when one is
        // open), so it is never empty.
        window.pre_present_notify();
        renderer.render(
            RenderLayer::rects_and_glyphs(&rects, &glyphs),
            Some(RenderLayer {
                shadows: &overlay_shadows,
                rects: &overlay_rects,
                round_rects: &overlay_round_rects,
                glyphs: &overlay_glyphs,
            }),
            0.0,
        );
        self.text.shape_cache.end_frame();
        self.text.ui_shape_cache.end_frame();
        // Drive the popup fade + panel slide to completion: while a transition
        // is in flight, request the next frame (event-driven redraws alone
        // wouldn't tick).
        if popup_animating || panel_animating {
            window.request_redraw();
        }
    }
}
