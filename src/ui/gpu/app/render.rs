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
use term_ui::{Bounds, Modified};

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
        // Column-decoration fade, tied to the SAME width tween (so it animates
        // over the same duration): transparent when collapsed (panel fully
        // hidden, only the opaque pill remains near the edge), opaque when open.
        let target_w = self.state.right.width();
        let fade = ((overlay_w - strip_w) / (target_w - strip_w).max(1.0)).clamp(0.0, 1.0);
        // Pager position: ease toward the focused page index (the horizontal
        // slide). Same pattern as the width tween — the model holds the discrete
        // focus, this chases it; `value(now)` is the continuous page position.
        let current_page = self.state.right.focus_index().unwrap_or(0);
        // While a swipe is active the gesture drives page_scroll directly (the
        // page follows the finger); otherwise the spring target chases the focused
        // page — hotkey/click paging AND the post-release snap settle.
        if !self.page_swipe.active {
            self.page_scroll.set_target(current_page as f32);
        }
        let page_scroll = self.page_scroll.value(now);
        let page_animating = self.page_scroll.animating();
        // Page viewport width = the overlay minus the 1px column border each side.
        let page_w = (overlay_w - 2.0).max(0.0);
        // The overlay is "on" unless it's the empty + collapsed + idle default.
        let show = !(right_empty && !right_visible && !panel_animating && dragging.is_none());
        let panels = show
            .then(|| panels_view::panel_manager_view(&self.state.right, expanded, page_scroll, page_w, fade));
        let pill = show.then(|| panels_view::pill_view(expanded, self.state.right.any_active()));
        let overlay_origin =
            Vec2::new((window_logical.x - overlay_w).max(0.0), HEADER_HEIGHT_LOGICAL);
        let overlay_size = Vec2::new(
            overlay_w,
            (window_logical.y - HEADER_HEIGHT_LOGICAL - FOOTER_HEIGHT_LOGICAL).max(0.0),
        );
        self.overlay.render_panels(
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
        // ── M2: draw each visible teammate pane's LIVE grid into its page slot ──
        // The term_ui pager above renders the frame + dots (its bg fill is dropped
        // so it can't cover these); here the coordinator positions the grids by
        // the same `page_scroll` math and draws them via `populate_panel` (R5).
        // Layering (within the overlay layer's fixed rects→round_rects→glyphs
        // order): an opaque page backdrop + the grid cells go into `overlay_rects`
        // (so the round-rect border frames them), grid text into `overlay_glyphs`;
        // both are clipped to the single-page viewport (off-edge neighbours
        // trimmed) and faded with the collapse animation.
        if show && expanded && page_w > 1.0 {
            let border = 1.0_f32;
            let content_origin = overlay_origin + Vec2::new(border, border);
            let page_h = (overlay_size.y - 2.0 * border - panels_view::STRIP_H).max(0.0);
            let clip = [
                content_origin.x,
                content_origin.y,
                content_origin.x + page_w,
                content_origin.y + page_h,
            ];
            let cell_w = (metrics.width_physical / sf).max(1.0);
            let cell_h = (metrics.height_physical / sf).max(1.0);
            let cols = (page_w / cell_w) as usize;
            let rows = (page_h / cell_h) as usize;
            let current = self.state.right.focus_index().unwrap_or(0);
            let n = self.state.right.len();
            let first = current.saturating_sub(1);
            let last = (current + 1).min(n.saturating_sub(1));
            // Resolve (index, pane) up front so the panels/registry borrows drop
            // before the mutable panes/text/atlas borrows below.
            let window: Vec<_> = (first..=last)
                .filter_map(|i| {
                    let panel = self.state.right.panels().get(i)?;
                    Some((i, self.child_sessions.pane_for(panel.id)?))
                })
                .collect();
            for (i, pane) in window {
                let Some(surface) = self.panes.get_mut(pane) else { continue };
                surface.resize(cols, rows);
                let page_x = content_origin.x + (i as f32 - page_scroll) * page_w;
                // Opaque backdrop (the overlay floats over the terminal).
                overlay_rects.push(RectInstance {
                    pos: [page_x, content_origin.y],
                    size: [page_w, page_h],
                    color: with_panel_alpha(panels_view::OVERLAY_BG, fade),
                    clip,
                });
                let snapshot = surface.emulator.snapshot();
                let rect = term_gpu::PanelRect::new(page_x, content_origin.y, page_w, page_h);
                let r0 = overlay_rects.len();
                let g0 = overlay_glyphs.len();
                populate_panel(
                    &snapshot,
                    rect,
                    &self.text.palette,
                    &mut self.text.font_system,
                    &mut self.text.swash_cache,
                    renderer.atlas_mut(),
                    &mut self.text.shape_cache,
                    FONT_SIZE,
                    sf,
                    metrics,
                    0.0,
                    &mut overlay_rects,
                    &mut overlay_glyphs,
                );
                // Clip the grid to the page viewport + fade with the collapse.
                for r in &mut overlay_rects[r0..] {
                    r.clip = clip;
                    r.color[3] *= fade;
                }
                for g in &mut overlay_glyphs[g0..] {
                    g.clip = clip;
                    g.color[3] *= fade;
                }
            }
        }
        self.panel_overlay_rect = show.then(|| Bounds::new(overlay_origin, overlay_size));
        // The pill is centred on the divider (the overlay's left edge) and
        // vertically centred in the overlay band — rendered OUTSIDE the faded
        // column so it stays opaque when the panel collapses.
        self.panel_toggle_zone = self.overlay.render_panel_pill(
            pill,
            overlay_origin.x,
            overlay_origin.y,
            overlay_size.y,
            &mut self.text.font_system,
            &mut self.text.swash_cache,
            renderer.atlas_mut(),
            &mut self.text.ui_shape_cache,
            sf,
            &mut overlay_round_rects,
            &mut overlay_glyphs,
        );

        // Popup overlay — all three popups render via the term_ui SECOND TREE.
        // The backend switch needs runtime data AppState doesn't carry (the
        // backend list + active/override ids), so it is built here via
        // popup_view::backend_view; history + settings come straight from
        // AppState via popup_view::popup_view. Whichever is open is reconciled
        // into the popup tree, measured with a min-width floor, centred with
        // place_centered, and painted into the overlay on top of the chrome (its
        // term_ui Block drop shadow flows through too). Popups are mutually
        // exclusive, so at most one is ever built.
        let popup: Option<Modified> = if self.state.backend_switch.is_visible() {
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
        // Drive the popup fade + panel slide + pager slide to completion: while a
        // transition is in flight, request the next frame (event-driven redraws
        // alone wouldn't tick).
        if popup_animating || panel_animating || page_animating {
            window.request_redraw();
        }
    }
}

/// Multiply a colour's alpha by `a` (RGB untouched) — bakes the collapse fade
/// into the coordinator-drawn grid backdrop.
fn with_panel_alpha(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * a]
}
