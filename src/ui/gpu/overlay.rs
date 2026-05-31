//! The overlay renderer: owns the two retained term_ui trees drawn on top of the
//! terminal grid — the chrome (header + footer) and the popup (history /
//! settings / backend switch) — plus the popup's open/close fade epoch. The
//! coordinator builds the views (it holds the AppState + backend data) and hands
//! them here; this runs the term_ui pipeline (reconcile → measure → place →
//! paint), bakes the fade alpha (R12), and appends the instances to the caller's
//! overlay buffers. Keeping the trees + the pipeline here is what lifts ~150
//! lines of `redraw` out of the coordinator.

use std::time::Instant;

use glam::Vec2;
use term_gpu::{
    FontSystem, GlyphAtlas, GlyphInstance, RectInstance, ShadowInstance, SwashCache, TextShapeCache,
};
use term_ui::{
    apply_overlay_alpha, build_root, free_subtree, measure, paint, place, place_centered,
    reconcile_root, Block, NodeId, PaintOutput, RetainedTree, SizeConstraint, Stack,
};

use crate::ui::chrome_labels;
use crate::ui::popup_anim::{popup_fade_alpha, step_popup_anim, PopupAnim};

pub(super) struct OverlayRenderer {
    chrome_tree: RetainedTree,
    chrome_root: Option<NodeId>,
    chrome_prev: Option<Stack>,
    chrome_scratch: PaintOutput,
    popup_tree: RetainedTree,
    popup_root: Option<NodeId>,
    popup_prev: Option<Block>,
    popup_scratch: PaintOutput,
    /// Open/close fade epoch (bucket 3-S); `None` when no fade is in flight, the
    /// alpha derived from it + the frame clock (R12).
    popup_anim: Option<PopupAnim>,
    /// The panels overlay (right teammates column) — a third retained tree,
    /// positioned (not centred) at the overlay rect each frame.
    panels_tree: RetainedTree,
    panels_root: Option<NodeId>,
    panels_prev: Option<Block>,
    panels_scratch: PaintOutput,
}

impl OverlayRenderer {
    pub(super) fn new() -> Self {
        Self {
            chrome_tree: RetainedTree::new(),
            chrome_root: None,
            chrome_prev: None,
            chrome_scratch: PaintOutput::default(),
            popup_tree: RetainedTree::new(),
            popup_root: None,
            popup_prev: None,
            popup_scratch: PaintOutput::default(),
            popup_anim: None,
            panels_tree: RetainedTree::new(),
            panels_root: None,
            panels_prev: None,
            panels_scratch: PaintOutput::default(),
        }
    }

    /// Reconcile + lay out (tight to `size`, placed at `origin`) + paint the
    /// panels overlay `view` into the caller's overlay buffers. `None` tears the
    /// retained tree down (overlay hidden + empty). Unlike the popup this is
    /// POSITIONED at the overlay rect, not centred, and carries no fade.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_panels(
        &mut self,
        view: Option<Block>,
        origin: Vec2,
        size: Vec2,
        fonts: &mut FontSystem,
        swash: &mut SwashCache,
        atlas: &mut GlyphAtlas,
        ui_shape: &mut TextShapeCache,
        sf: f32,
        out_rects: &mut Vec<RectInstance>,
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
        out_glyphs.extend_from_slice(&self.chrome_scratch.glyphs);
        self.chrome_tree
            .resolve_widget(chrome_labels::session_widget_id())
            .map(|nid| {
                let b = self.chrome_tree.node(nid).bounds;
                (b.origin.x, b.right())
            })
    }

    /// Advance the open/close fade epoch (R12), then reconcile / keep-alive + lay
    /// out (min-width floored, centred) + paint the popup `view` into the
    /// caller's overlay buffers at the eased alpha. `None` = no popup; during a
    /// fade-OUT the retained tree is kept alive (and re-painted at the decreasing
    /// alpha) until the fade ends. Returns whether the fade is still animating
    /// (so the caller re-requests a redraw to drive it).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_popup(
        &mut self,
        view: Option<Block>,
        window: Vec2,
        now: Instant,
        fade_secs: f32,
        min_width: f32,
        fonts: &mut FontSystem,
        swash: &mut SwashCache,
        atlas: &mut GlyphAtlas,
        ui_shape: &mut TextShapeCache,
        sf: f32,
        out_shadows: &mut Vec<ShadowInstance>,
        out_rects: &mut Vec<RectInstance>,
        out_glyphs: &mut Vec<GlyphInstance>,
    ) -> bool {
        let visible = view.is_some();
        self.popup_anim = step_popup_anim(self.popup_anim, visible, now);
        let (alpha, animating) = popup_fade_alpha(self.popup_anim, now, fade_secs);

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
            // No popup + no fade — release the retained tree and reset the epoch.
            if let Some(root) = self.popup_root.take() {
                free_subtree(&mut self.popup_tree, root);
            }
            self.popup_prev = None;
            self.popup_anim = None;
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
            out_glyphs.extend_from_slice(&self.popup_scratch.glyphs);
        }
        animating
    }
}
