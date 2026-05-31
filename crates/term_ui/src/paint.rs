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
    ShadowInstance, Style, SwashCache, TextShapeCache, Weight,
};

use crate::arena::{BlockStyle, NodeKind, RetainedTree, TextStyle};
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
    let node = tree.node(id);
    let bounds = node.bounds;
    let kind = node.kind.clone();
    let widget_id = node.widget_id;
    let children = node.children.clone();

    if let Some(wid) = widget_id {
        out.hitboxes.push((bounds, wid));
    }

    match kind {
        NodeKind::Spacer(_) => {}
        NodeKind::Block(style) => {
            // §11: a drop shadow is emitted UNDER the bg rect (pushed first so
            // the opaque bg covers the saturated SDF centre, leaving the halo).
            // `None` / fully-transparent shadow emits nothing — the case for
            // every plain chrome Block. paint_cpu (R4 gate) stays shadow-free.
            if let Some(shadow) = block_shadow(bounds, &style) {
                out.shadows.push(shadow);
            }
            if style.background[3] > 0.0 {
                out.rects.push(rect(bounds, style.background));
            }
            if style.border_width > 0.0 && style.border_color[3] > 0.0 {
                push_border(out, bounds, style.border_width, style.border_color);
            }
        }
        NodeKind::Modified(modifier) => paint_modifier(out, bounds, &modifier),
        NodeKind::Text(style) => {
            let baseline_y = bounds.origin.y + bounds.size.y * BASELINE_RATIO;
            let (weight, css_style) = text_attrs(&style);
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
                style.color,
            );
        }
        NodeKind::Stack(_) => {}
    }

    for child in children {
        paint(tree, child, out, atlas, fonts, swash, shape, scale_factor);
    }
}

/// Shrink `b` by `insets` (origin moves in by the leading insets, size shrinks
/// by the total; clamped to ≥ 0).
fn inset_bounds(b: Bounds, insets: Insets) -> Bounds {
    Bounds::new(b.origin + insets.top_left(), (b.size - insets.total()).max(Vec2::ZERO))
}

/// Fold a [`Modifier`] chain in order, emitting its decorations at the box
/// bounds AT THAT POINT in the chain (R-style box model, order honoured). Layout
/// ops shrink the running bounds; draw ops emit a [`RoundRectInstance`] /
/// [`ShadowInstance`]; `corner_radius` sets the rounding for subsequent draws.
/// The child is painted separately (by the generic recursion) at its placed
/// bounds, which equal the fully-inset `b` here.
fn paint_modifier(out: &mut PaintOutput, node_bounds: Bounds, modifier: &Modifier) {
    let mut b = node_bounds;
    let mut corner = 0.0_f32;
    for op in &modifier.ops {
        match *op {
            Mod::Margin(i) | Mod::Padding(i) => b = inset_bounds(b, i),
            Mod::CornerRadius(r) => corner = r,
            Mod::Background(color) => {
                if color[3] > 0.0 {
                    out.round_rects.push(RoundRectInstance::fill(
                        b.origin.into(),
                        b.size.into(),
                        color,
                        corner,
                    ));
                }
            }
            Mod::Border { width, color } => {
                if width > 0.0 && color[3] > 0.0 {
                    out.round_rects.push(RoundRectInstance::new(
                        b.origin.into(),
                        b.size.into(),
                        [0.0; 4],
                        color,
                        width,
                        corner,
                    ));
                }
                // Content sits inside the border.
                b = inset_bounds(b, Insets::all(width));
            }
            Mod::Shadow(s) => {
                if s.color[3] > 0.0 {
                    out.shadows.push(ShadowInstance {
                        pos: b.origin.into(),
                        size: b.size.into(),
                        blur_radius: s.blur_radius,
                        corner_radius: s.corner_radius,
                        offset: s.offset,
                        color: s.color,
                    });
                }
            }
        }
    }
}

/// One painted glyph's CPU-computable identity + geometry, for the R4 gate.
/// `cache_key` is cosmic-text's atlas-independent glyph identity; there are NO
/// atlas UVs and NO frame counters here, so it is path-independent.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GlyphRecord {
    pub cache_key: CacheKey,
    pub color: [f32; 4],
}

/// One painted rect's CPU-computable geometry + color, for the R4 gate.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RectRecord {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
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
    let node = tree.node(id);
    let bounds = node.bounds;
    let kind = node.kind.clone();
    let widget_id = node.widget_id;
    let children = node.children.clone();

    if let Some(wid) = widget_id {
        out.hitboxes.push((bounds, wid));
    }

    match kind {
        NodeKind::Spacer(_) | NodeKind::Stack(_) => {}
        NodeKind::Block(style) => {
            if style.background[3] > 0.0 {
                out.rects.push(RectRecord {
                    origin: bounds.origin.into(),
                    size: bounds.size.into(),
                    color: style.background,
                });
            }
            if style.border_width > 0.0 && style.border_color[3] > 0.0 {
                for b in border_rects(bounds, style.border_width) {
                    out.rects.push(RectRecord {
                        origin: b.origin.into(),
                        size: b.size.into(),
                        color: style.border_color,
                    });
                }
            }
        }
        NodeKind::Modified(modifier) => {
            // CPU geometry only (R4 gate): a RectRecord per background/border at
            // the folded bounds; rounding + shadows are bucket-3-S, excluded.
            let mut b = bounds;
            for op in &modifier.ops {
                match *op {
                    Mod::Margin(i) | Mod::Padding(i) => b = inset_bounds(b, i),
                    Mod::CornerRadius(_) | Mod::Shadow(_) => {}
                    Mod::Background(color) => {
                        if color[3] > 0.0 {
                            out.rects.push(RectRecord {
                                origin: b.origin.into(),
                                size: b.size.into(),
                                color,
                            });
                        }
                    }
                    Mod::Border { width, color } => {
                        if width > 0.0 && color[3] > 0.0 {
                            out.rects.push(RectRecord {
                                origin: b.origin.into(),
                                size: b.size.into(),
                                color,
                            });
                        }
                        b = inset_bounds(b, Insets::all(width));
                    }
                }
            }
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
        paint_cpu(tree, child, out, fonts, shape, scale_factor);
    }
}

fn text_attrs(style: &TextStyle) -> (Weight, Style) {
    let weight = Weight(style.weight);
    let css_style = if style.italic { Style::Italic } else { Style::Normal };
    (weight, css_style)
}

/// The drop-shadow instance a [`crate::view::Block`] emits beneath its
/// background, or `None` when the style carries no visible shadow (`shadow:
/// None`, or a fully transparent colour). Split out of [`paint`] so the
/// shadow-emit mapping is headlessly testable — the live `paint` path needs a
/// GPU `GlyphAtlas`, but this pure geometry does not.
pub fn block_shadow(bounds: Bounds, style: &BlockStyle) -> Option<ShadowInstance> {
    let sh = style.shadow?;
    if sh.color[3] <= 0.0 {
        return None;
    }
    Some(ShadowInstance {
        pos: bounds.origin.into(),
        size: bounds.size.into(),
        blur_radius: sh.blur_radius,
        corner_radius: sh.corner_radius,
        offset: sh.offset,
        color: sh.color,
    })
}

fn rect(bounds: Bounds, color: [f32; 4]) -> RectInstance {
    RectInstance {
        pos: bounds.origin.into(),
        size: bounds.size.into(),
        color,
    }
}

/// Four thin rects forming a border inside `bounds` (1px-thick edges scaled by
/// `width`). Drawn over the background.
fn border_rects(bounds: Bounds, width: f32) -> [Bounds; 4] {
    let o = bounds.origin;
    let s = bounds.size;
    [
        // top
        Bounds::new(o, Vec2::new(s.x, width)),
        // bottom
        Bounds::new(Vec2::new(o.x, o.y + s.y - width), Vec2::new(s.x, width)),
        // left
        Bounds::new(o, Vec2::new(width, s.y)),
        // right
        Bounds::new(Vec2::new(o.x + s.x - width, o.y), Vec2::new(width, s.y)),
    ]
}

fn push_border(out: &mut PaintOutput, bounds: Bounds, width: f32, color: [f32; 4]) {
    for b in border_rects(bounds, width) {
        out.rects.push(rect(b, color));
    }
}
