//! The overlay renderer: owns the two retained term_ui trees drawn on top of the
//! terminal grid — the chrome (header + footer) and the popup (history /
//! settings / backend switch) — plus the popup's open/close fade epoch. The
//! coordinator builds the views (it holds the AppState + backend data) and hands
//! them here; this runs the term_ui pipeline (reconcile → measure → place →
//! paint), bakes the fade alpha (R12), and appends the instances to the caller's
//! overlay buffers. Keeping the trees + the pipeline here is what lifts ~150
//! lines of `redraw` out of the coordinator.

use std::time::{Duration, Instant};

use glam::Vec2;
use term_gpu::{
    FontSystem, GlyphAtlas, GlyphInstance, RectInstance, RoundRectInstance, ShadowInstance,
    SwashCache, TextShapeCache,
};
use term_ui::{
    apply_overlay_alpha, build_root, free_subtree, measure, paint, place, place_centered,
    reconcile_root, Animation, Bounds, Interpolator, Modified, NodeId, PaintOutput, RetainedTree,
    SizeConstraint, Stack,
};

use crate::ui::chrome_labels;

pub(super) struct OverlayRenderer {
    chrome_tree: RetainedTree,
    chrome_root: Option<NodeId>,
    chrome_prev: Option<Stack>,
    chrome_scratch: PaintOutput,
    popup_tree: RetainedTree,
    popup_root: Option<NodeId>,
    popup_prev: Option<Modified>,
    popup_scratch: PaintOutput,
    /// Open/close fade alpha (bucket 3-S): `0 → 1` open, `1 → 0` close. The alpha
    /// is `value(now)` each frame, never stored resolved (R12).
    popup_alpha: Animation<f32>,
    /// The panels overlay (right teammates column) — a third retained tree,
    /// positioned (not centred) at the overlay rect each frame.
    panels_tree: RetainedTree,
    panels_root: Option<NodeId>,
    panels_prev: Option<Modified>,
    panels_scratch: PaintOutput,
    /// The collapse/expand pill — a FOURTH retained tree, rendered OUTSIDE the
    /// faded panels column so it stays opaque, centred on the divider.
    pill_tree: RetainedTree,
    pill_root: Option<NodeId>,
    pill_prev: Option<Modified>,
    pill_scratch: PaintOutput,
}

impl OverlayRenderer {
    pub(super) fn new(fade_dur: Duration) -> Self {
        Self {
            chrome_tree: RetainedTree::new(),
            chrome_root: None,
            chrome_prev: None,
            chrome_scratch: PaintOutput::default(),
            popup_tree: RetainedTree::new(),
            popup_root: None,
            popup_prev: None,
            popup_scratch: PaintOutput::default(),
            popup_alpha: Animation::settled(0.0, Instant::now(), fade_dur, Interpolator::EaseOut),
            panels_tree: RetainedTree::new(),
            panels_root: None,
            panels_prev: None,
            panels_scratch: PaintOutput::default(),
            pill_tree: RetainedTree::new(),
            pill_root: None,
            pill_prev: None,
            pill_scratch: PaintOutput::default(),
        }
    }

    /// Reconcile + measure (loose, intrinsic) + place the pill CENTRED on
    /// `divider_x` and vertically centred in the band `[band_y, band_y + band_h]`
    /// + paint it (opaque) into the caller's overlay buffers. `None` tears the
    /// pill tree down. Returns the pill's laid-out bounds (its hit-zone).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_panel_pill(
        &mut self,
        view: Option<Modified>,
        divider_x: f32,
        band_y: f32,
        band_h: f32,
        fonts: &mut FontSystem,
        swash: &mut SwashCache,
        atlas: &mut GlyphAtlas,
        ui_shape: &mut TextShapeCache,
        sf: f32,
        out_round_rects: &mut Vec<RoundRectInstance>,
        out_glyphs: &mut Vec<GlyphInstance>,
    ) -> Option<Bounds> {
        let Some(view) = view else {
            if let Some(root) = self.pill_root.take() {
                free_subtree(&mut self.pill_tree, root);
            }
            self.pill_prev = None;
            return None;
        };
        let root = match self.pill_root {
            Some(root) => {
                let prev = self.pill_prev.take().expect("pill_prev present once built");
                reconcile_root(&mut self.pill_tree, root, &prev, &view);
                root
            }
            None => build_root(&mut self.pill_tree, &view),
        };
        self.pill_root = Some(root);
        self.pill_prev = Some(view);
        // Measure intrinsic, then centre on the divider + the band.
        let size = measure(
            &mut self.pill_tree,
            root,
            SizeConstraint::loose(Vec2::new(f32::INFINITY, band_h.max(0.0))),
            fonts,
            ui_shape,
            sf,
        );
        let origin = Vec2::new(divider_x - size.x * 0.5, band_y + (band_h - size.y) * 0.5);
        place(&mut self.pill_tree, root, origin);
        self.pill_scratch.clear();
        paint(&self.pill_tree, root, &mut self.pill_scratch, atlas, fonts, swash, ui_shape, sf);
        out_round_rects.extend_from_slice(&self.pill_scratch.round_rects);
        out_glyphs.extend_from_slice(&self.pill_scratch.glyphs);
        Some(self.pill_tree.node(root).bounds)
    }

    /// Reconcile + lay out (tight to `size`, placed at `origin`) + paint the
    /// panels overlay `view` into the caller's overlay buffers. `None` tears the
    /// retained tree down (overlay hidden + empty). Unlike the popup this is
    /// POSITIONED at the overlay rect, not centred, and carries no fade. Returns
    /// the toggle/indicator button's laid-out bounds (resolved from the tree by
    /// its `WidgetId`) so the coordinator can hit-test clicks on it; `None` when
    /// nothing was rendered.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_panels(
        &mut self,
        view: Option<Modified>,
        origin: Vec2,
        size: Vec2,
        fonts: &mut FontSystem,
        swash: &mut SwashCache,
        atlas: &mut GlyphAtlas,
        ui_shape: &mut TextShapeCache,
        sf: f32,
        out_rects: &mut Vec<RectInstance>,
        out_round_rects: &mut Vec<RoundRectInstance>,
        out_glyphs: &mut Vec<GlyphInstance>,
    ) {
        let Some(view) = view else {
            if let Some(root) = self.panels_root.take() {
                free_subtree(&mut self.panels_tree, root);
            }
            self.panels_prev = None;
            return;
        };
        let root = match self.panels_root {
            Some(root) => {
                let prev = self.panels_prev.take().expect("panels_prev present once built");
                reconcile_root(&mut self.panels_tree, root, &prev, &view);
                root
            }
            None => build_root(&mut self.panels_tree, &view),
        };
        self.panels_root = Some(root);
        self.panels_prev = Some(view);
        measure(&mut self.panels_tree, root, SizeConstraint::tight(size), fonts, ui_shape, sf);
        place(&mut self.panels_tree, root, origin);
        self.panels_scratch.clear();
        paint(&self.panels_tree, root, &mut self.panels_scratch, atlas, fonts, swash, ui_shape, sf);
        out_rects.extend_from_slice(&self.panels_scratch.rects);
        out_round_rects.extend_from_slice(&self.panels_scratch.round_rects);
        out_glyphs.extend_from_slice(&self.panels_scratch.glyphs);
    }

    /// Reconcile + lay out (tight to `window`) + paint the chrome `view` into the
    /// caller's overlay rects/glyphs, and return the session-click hot-zone
    /// (x-range) resolved from the laid-out tree (the tagged "Session: …" run).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_chrome(
        &mut self,
        view: Stack,
        window: Vec2,
        fonts: &mut FontSystem,
        swash: &mut SwashCache,
        atlas: &mut GlyphAtlas,
        ui_shape: &mut TextShapeCache,
        sf: f32,
        out_rects: &mut Vec<RectInstance>,
        out_round_rects: &mut Vec<RoundRectInstance>,
        out_glyphs: &mut Vec<GlyphInstance>,
    ) -> Option<(f32, f32)> {
        let root = match self.chrome_root {
            Some(root) => {
                let prev = self.chrome_prev.take().expect("chrome_prev present once built");
                reconcile_root(&mut self.chrome_tree, root, &prev, &view);
                root
            }
            None => build_root(&mut self.chrome_tree, &view),
        };
        self.chrome_root = Some(root);
        self.chrome_prev = Some(view);
        measure(&mut self.chrome_tree, root, SizeConstraint::tight(window), fonts, ui_shape, sf);
        place(&mut self.chrome_tree, root, Vec2::ZERO);
        self.chrome_scratch.clear();
        paint(&self.chrome_tree, root, &mut self.chrome_scratch, atlas, fonts, swash, ui_shape, sf);
        out_rects.extend_from_slice(&self.chrome_scratch.rects);
        out_round_rects.extend_from_slice(&self.chrome_scratch.round_rects);
        out_glyphs.extend_from_slice(&self.chrome_scratch.glyphs);
        self.chrome_tree
            .resolve_widget(chrome_labels::session_widget_id())
            .map(|nid| {
                let b = self.chrome_tree.node(nid).bounds;
                (b.origin.x, b.right())
            })
    }

    /// Drive the open/close fade tween (R12), then reconcile / keep-alive + lay
    /// out (min-width floored, centred) + paint the popup `view` into the
    /// caller's overlay buffers at the eased alpha. `None` = no popup; during a
    /// fade-OUT the retained tree is kept alive (and re-painted at the decreasing
    /// alpha) until the fade ends. Returns whether the fade is still animating
    /// (so the caller re-requests a redraw to drive it).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_popup(
        &mut self,
        view: Option<Modified>,
        window: Vec2,
        now: Instant,
        min_width: f32,
        fonts: &mut FontSystem,
        swash: &mut SwashCache,
        atlas: &mut GlyphAtlas,
        ui_shape: &mut TextShapeCache,
        sf: f32,
        out_shadows: &mut Vec<ShadowInstance>,
        out_rects: &mut Vec<RectInstance>,
        out_round_rects: &mut Vec<RoundRectInstance>,
        out_glyphs: &mut Vec<GlyphInstance>,
    ) -> bool {
        // Drive the fade toward open (1.0) or closed (0.0); the alpha is the
        // tween's derived value (R12). `retarget` is a no-op once it's there.
        self.popup_alpha.retarget(if view.is_some() { 1.0 } else { 0.0 }, now);
        let alpha = self.popup_alpha.value(now);
        let animating = self.popup_alpha.animating(now);

        // The root to paint: the live popup (reconciled), or — during a fade-OUT,
        // when the store is already Hidden — the retained tree kept alive.
        let root_to_paint: Option<NodeId> = if let Some(view) = view {
            let root = match self.popup_root {
                Some(root) => {
                    let prev = self.popup_prev.take().expect("popup_prev present once built");
                    reconcile_root(&mut self.popup_tree, root, &prev, &view);
                    root
                }
                None => build_root(&mut self.popup_tree, &view),
            };
            self.popup_root = Some(root);
            self.popup_prev = Some(view);
            Some(root)
        } else if animating {
            self.popup_root
        } else {
            // No popup + no fade — release the retained tree (the tween rests
            // at 0, ready for the next open).
            if let Some(root) = self.popup_root.take() {
                free_subtree(&mut self.popup_tree, root);
            }
            self.popup_prev = None;
            None
        };
        if let Some(root) = root_to_paint {
            measure(
                &mut self.popup_tree,
                root,
                SizeConstraint::new(Vec2::new(min_width, 0.0), window),
                fonts,
                ui_shape,
                sf,
            );
            place_centered(&mut self.popup_tree, root, window);
            self.popup_scratch.clear();
            paint(&self.popup_tree, root, &mut self.popup_scratch, atlas, fonts, swash, ui_shape, sf);
            // Bake the fade alpha into the popup's instances only (the chrome
            // beneath, already merged, keeps full opacity).
            if alpha < 1.0 {
                apply_overlay_alpha(&mut self.popup_scratch, alpha);
            }
            out_shadows.extend_from_slice(&self.popup_scratch.shadows);
            out_rects.extend_from_slice(&self.popup_scratch.rects);
            out_round_rects.extend_from_slice(&self.popup_scratch.round_rects);
            out_glyphs.extend_from_slice(&self.popup_scratch.glyphs);
        }
        animating
    }
}
