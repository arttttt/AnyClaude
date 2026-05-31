//! PAINT pass (design §5 paint contract). Walks the arena at placed origins
//! and emits term_gpu instances into caller-owned `Vec`s. Index-based free
//! function; the text infra (`atlas`, `FontSystem`, `SwashCache`,
//! `TextShapeCache`) are separate `&mut` params, disjoint from `tree` (§14).
//!
//! Two flavors:
//! - [`paint`] is the live GPU path: it rasterizes glyphs through the atlas via
//!   `term_gpu::push_label` and emits `GlyphInstance`s with real atlas UVs. The
//!   toy example uses this.
//! - [`paint_cpu`] is the headless path for the R4 property test: it computes
//!   the same geometry/color and the glyph **identity** (cosmic-text `CacheKey`)
//!   WITHOUT any atlas/GPU, so two reconcile paths can be compared on
//!   CPU-computable output only (no UVs, no frame counters).

use glam::Vec2;

use term_gpu::{
    push_label, CacheKey, FontSystem, GlyphAtlas, GlyphInstance, RectInstance, RoundRectInstance,
    ShadowInstance, Style, SwashCache, TextShapeCache, Weight, NO_CLIP,
};

use crate::arena::{NodeKind, RetainedTree, TextStyle};
use crate::geometry::{Bounds, Insets};
use crate::id::{NodeId, WidgetId};
use crate::modifier::{Mod, Modifier};

/// term_gpu's `push_label` anchors text by **baseline**, computed by callers as
/// `top + line_height * BASELINE_RATIO`. We use the same 0.75 ratio the label
/// module documents, so the toy and the live chrome agree.
const BASELINE_RATIO: f32 = 0.75;

/// Output buffers for one frame's base layer (owned by the caller / coordinator
/// and reused across frames as scratch — §14).
#[derive(Default)]
pub struct PaintOutput {
    pub rects: Vec<RectInstance>,
    /// Rounded-box decorations (modifier backgrounds / borders) — drawn over the
    /// sharp `rects` and under the `glyphs`.
    pub round_rects: Vec<RoundRectInstance>,
    pub glyphs: Vec<GlyphInstance>,
    pub shadows: Vec<ShadowInstance>,
    /// Per-frame hit geometry (bucket 2): topmost-wins in z-order is the
    /// caller's concern; Phase A just records `(bounds, id)` in paint order.
    pub hitboxes: Vec<(Bounds, WidgetId)>,
}

impl PaintOutput {
    pub fn clear(&mut self) {
        self.rects.clear();
        self.round_rects.clear();
        self.glyphs.clear();
        self.shadows.clear();
        self.hitboxes.clear();
    }
}

/// LIVE paint: emit instances (with real atlas UVs) for the subtree at `id`.
#[allow(clippy::too_many_arguments)]
pub fn paint(
    tree: &RetainedTree,
    id: NodeId,
    out: &mut PaintOutput,
    atlas: &mut GlyphAtlas,
    fonts: &mut FontSystem,
    swash: &mut SwashCache,
    shape: &mut TextShapeCache,
    scale_factor: f32,
) {
    paint_inner(tree, id, out, atlas, fonts, swash, shape, scale_factor, 1.0, NO_CLIP);
}

/// Recursive paint with an inherited subtree `alpha` (a `Mod::Alpha` multiplies
/// it for the node + its descendants — graphicsLayer-style opacity) and `clip`
/// rect (a `Mod::Clip` intersects it — graphicsLayer-style clipping). Both are
/// threaded into every emitted instance.
#[allow(clippy::too_many_arguments)]
fn paint_inner(
    tree: &RetainedTree,
    id: NodeId,
    out: &mut PaintOutput,
    atlas: &mut GlyphAtlas,
    fonts: &mut FontSystem,
    swash: &mut SwashCache,
    shape: &mut TextShapeCache,
    scale_factor: f32,
    alpha: f32,
    clip: [f32; 4],
) {
    let node = tree.node(id);
    let bounds = node.bounds;
    let kind = node.kind.clone();
    let widget_id = node.widget_id;
    let children = node.children.clone();

    if let Some(wid) = widget_id {
        out.hitboxes.push((bounds, wid));
    }

    // The alpha / clip carried into this node's children (a Modified's `Alpha`
    // ops multiply the alpha; a `Clip` op intersects the clip — both for the
    // whole subtree).
    let mut child_alpha = alpha;
    let mut child_clip = clip;
    match kind {
        NodeKind::Spacer(_) | NodeKind::Stack(_) => {}
        NodeKind::Modified(modifier) => {
            child_alpha = alpha * modifier.total_alpha();
            child_clip = paint_modifier(out, bounds, &modifier, child_alpha, clip);
        }
        NodeKind::Text(style) => {
            let baseline_y = bounds.origin.y + bounds.size.y * BASELINE_RATIO;
            let (weight, css_style) = text_attrs(&style);
            let start = out.glyphs.len();
            push_label(
                fonts,
                swash,
                atlas,
                shape,
                &mut out.glyphs,
                &style.text,
                bounds.origin.x,
                baseline_y,
                style.font_size,
                scale_factor,
                weight,
                css_style,
                with_alpha(style.color, alpha),
            );
            // Text isn't a clipper; its glyphs inherit the ancestor clip.
            if clip != NO_CLIP {
                for g in &mut out.glyphs[start..] {
                    g.clip = clip;
                }
            }
        }
    }

    for child in children {
        paint_inner(
            tree, child, out, atlas, fonts, swash, shape, scale_factor, child_alpha, child_clip,
        );
    }
}

/// Shrink `b` by `insets` (origin moves in by the leading insets, size shrinks
/// by the total; clamped to ≥ 0).
fn inset_bounds(b: Bounds, insets: Insets) -> Bounds {
    Bounds::new(b.origin + insets.top_left(), (b.size - insets.total()).max(Vec2::ZERO))
}

/// Multiply a colour's alpha (RGB untouched).
fn with_alpha(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * a]
}

/// Intersect a clip rect `[min_x, min_y, max_x, max_y]` with a bounds box —
/// the fold a `Mod::Clip` applies as it descends the tree.
fn intersect_clip(clip: [f32; 4], b: Bounds) -> [f32; 4] {
    [
        clip[0].max(b.origin.x),
        clip[1].max(b.origin.y),
        clip[2].min(b.origin.x + b.size.x),
        clip[3].min(b.origin.y + b.size.y),
    ]
}

/// Fold a [`Modifier`] chain in order, emitting its decorations at the box
/// bounds AT THAT POINT in the chain (R-style box model, order honoured). Layout
/// ops shrink the running bounds; draw ops emit a [`RoundRectInstance`] /
/// [`ShadowInstance`] clipped to the running `clip`; `corner_radius` sets the
/// rounding for subsequent draws; `Mod::Clip` intersects the running clip with
/// the running bounds. The child is painted separately (by the generic
/// recursion) at its placed bounds; the returned clip is what it inherits.
fn paint_modifier(
    out: &mut PaintOutput,
    node_bounds: Bounds,
    modifier: &Modifier,
    alpha: f32,
    inherited_clip: [f32; 4],
) -> [f32; 4] {
    let mut b = node_bounds;
    let mut corner = 0.0_f32;
    let mut clip = inherited_clip;
    for op in &modifier.ops {
        match *op {
            Mod::Margin(i) | Mod::Padding(i) => b = inset_bounds(b, i),
            // Offset is applied in `place` (shifts node bounds); Alpha is folded
            // into `alpha` by the caller — both no-ops in the decoration loop.
            Mod::Offset(_) | Mod::Alpha(_) => {}
            Mod::Clip => clip = intersect_clip(clip, b),
            Mod::CornerRadius(r) => corner = r,
            Mod::Background(color) => {
                let color = with_alpha(color, alpha);
                if color[3] > 0.0 {
                    let mut ri =
                        RoundRectInstance::fill(b.origin.into(), b.size.into(), color, corner);
                    ri.clip = clip;
                    out.round_rects.push(ri);
                }
            }
            Mod::Border { width, color } => {
                let color = with_alpha(color, alpha);
                if width > 0.0 && color[3] > 0.0 {
                    let mut ri = RoundRectInstance::new(
                        b.origin.into(),
                        b.size.into(),
                        [0.0; 4],
                        color,
                        width,
                        corner,
                    );
                    ri.clip = clip;
                    out.round_rects.push(ri);
                }
                // Content sits inside the border.
                b = inset_bounds(b, Insets::all(width));
            }
            Mod::Shadow(s) => {
                // Shadow is a soft halo extending beyond the bounds; Mod::Clip
                // does not clip it (no consumer needs it).
                let color = with_alpha(s.color, alpha);
                if color[3] > 0.0 {
                    out.shadows.push(ShadowInstance {
                        pos: b.origin.into(),
                        size: b.size.into(),
                        blur_radius: s.blur_radius,
                        corner_radius: s.corner_radius,
                        offset: s.offset,
                        color,
                    });
                }
            }
        }
    }
    clip
}

/// One painted glyph's CPU-computable identity + geometry, for the R4 gate.
/// `cache_key` is cosmic-text's atlas-independent glyph identity; there are NO
/// atlas UVs and NO frame counters here, so it is path-independent.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GlyphRecord {
    pub cache_key: CacheKey,
    pub color: [f32; 4],
}

/// One painted rect's CPU-computable geometry + color + clip, for the R4 gate.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RectRecord {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
    /// The clip rect `[min_x, min_y, max_x, max_y]` in effect when this rect was
    /// emitted ([`NO_CLIP`] when unclipped) — mirrors the live path's per-
    /// instance clip so the fold is testable headlessly.
    pub clip: [f32; 4],
}

/// CPU-comparable paint output for the R4 property test. Holds only geometry,
/// color, and glyph identity — explicitly NO atlas UVs and NO frame counters.
#[derive(Default, PartialEq, Debug)]
pub struct CpuPaint {
    pub rects: Vec<RectRecord>,
    pub glyphs: Vec<GlyphRecord>,
    pub hitboxes: Vec<(Bounds, WidgetId)>,
}

/// HEADLESS paint: compute the same geometry/color + per-glyph `CacheKey` as
/// [`paint`], but WITHOUT an atlas/GPU. Used by the R4 property test to compare
/// rebuild-from-scratch vs incremental on CPU-computable output only.
pub fn paint_cpu(
    tree: &RetainedTree,
    id: NodeId,
    out: &mut CpuPaint,
    fonts: &mut FontSystem,
    shape: &mut TextShapeCache,
    scale_factor: f32,
) {
    paint_cpu_inner(tree, id, out, fonts, shape, scale_factor, NO_CLIP);
}

#[allow(clippy::too_many_arguments)]
fn paint_cpu_inner(
    tree: &RetainedTree,
    id: NodeId,
    out: &mut CpuPaint,
    fonts: &mut FontSystem,
    shape: &mut TextShapeCache,
    scale_factor: f32,
    inherited_clip: [f32; 4],
) {
    let node = tree.node(id);
    let bounds = node.bounds;
    let kind = node.kind.clone();
    let widget_id = node.widget_id;
    let children = node.children.clone();

    if let Some(wid) = widget_id {
        out.hitboxes.push((bounds, wid));
    }

    let mut child_clip = inherited_clip;
    match kind {
        NodeKind::Spacer(_) | NodeKind::Stack(_) => {}
        NodeKind::Modified(modifier) => {
            // CPU geometry only (R4 gate): a RectRecord per background/border at
            // the folded bounds + folded clip; rounding + shadows are
            // bucket-3-S, excluded.
            let mut b = bounds;
            let mut clip = inherited_clip;
            for op in &modifier.ops {
                match *op {
                    Mod::Margin(i) | Mod::Padding(i) => b = inset_bounds(b, i),
                    Mod::CornerRadius(_) | Mod::Shadow(_) | Mod::Offset(_) | Mod::Alpha(_) => {}
                    Mod::Clip => clip = intersect_clip(clip, b),
                    Mod::Background(color) => {
                        if color[3] > 0.0 {
                            out.rects.push(RectRecord {
                                origin: b.origin.into(),
                                size: b.size.into(),
                                color,
                                clip,
                            });
                        }
                    }
                    Mod::Border { width, color } => {
                        if width > 0.0 && color[3] > 0.0 {
                            out.rects.push(RectRecord {
                                origin: b.origin.into(),
                                size: b.size.into(),
                                color,
                                clip,
                            });
                        }
                        b = inset_bounds(b, Insets::all(width));
                    }
                }
            }
            child_clip = clip;
        }
        NodeKind::Text(style) => {
            let baseline_y = bounds.origin.y + bounds.size.y * BASELINE_RATIO;
            let (weight, css_style) = text_attrs(&style);
            // Shape once and compute the per-glyph CacheKey exactly as
            // `push_label` would (same physical origin + scale=1.0), but never
            // touch the atlas. This is the atlas-independent glyph identity.
            let sf = scale_factor;
            let origin_x_phys = bounds.origin.x * sf;
            let baseline_y_phys = (baseline_y * sf).round();
            let shaped = shape.shape(
                fonts,
                &style.text,
                style.font_size,
                sf,
                None,
                weight,
                css_style,
            );
            for line in &shaped.lines {
                for glyph in &line.glyphs {
                    let physical = glyph.physical((origin_x_phys, baseline_y_phys), 1.0);
                    out.glyphs.push(GlyphRecord {
                        cache_key: physical.cache_key,
                        color: style.color,
                    });
                }
            }
        }
    }

    for child in children {
        paint_cpu_inner(tree, child, out, fonts, shape, scale_factor, child_clip);
    }
}

fn text_attrs(style: &TextStyle) -> (Weight, Style) {
    let weight = Weight(style.weight);
    let css_style = if style.italic { Style::Italic } else { Style::Normal };
    (weight, css_style)
}
