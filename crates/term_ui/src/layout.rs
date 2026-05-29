//! Flex-lite layout engine (design §5): constraints-down / sizes-up
//! (`measure`), then positions-down (`place`). Index-based free functions over
//! the arena — NOT `&mut self` recursive methods. Each pass copies out the
//! small per-node data (kind, child list), releases the `tree` borrow, then
//! recurses per child with a fresh borrow (§14). `FontSystem` /
//! `TextShapeCache` are separate `&mut` params, disjoint from `tree`.
//!
//! This is Flutter/warpui-style Flex-lite, NOT CSS flexbox, NOT Taffy. The
//! main-axis modes are `Start/Center/End/SpaceBetween`; cross-axis modes are
//! `Start/Center/End/Stretch`; per-child main sizing is `Auto/Fixed/Flex/Fill`.

use glam::Vec2;

use term_gpu::{FontSystem, Style, TextShapeCache, Weight};

use crate::arena::{NodeKind, RetainedTree, TextStyle};
use crate::geometry::{Axis, Bounds, CrossAxis, MainAxis, SizeConstraint, Sizing};
use crate::id::NodeId;

/// Line height for a text run at `font_size`, derived from the face metrics
/// (`cell_height` = ascent + descent + line_gap). Falls back to `font_size *
/// 1.3` (cosmic-text's default line factor in `shape_text_inline`) if face
/// resolution fails. Returned in **logical** pixels.
pub fn line_height(
    fonts: &mut FontSystem,
    shape: &mut TextShapeCache,
    style: &TextStyle,
    scale_factor: f32,
) -> f32 {
    let weight = Weight(style.weight);
    let css_style = if style.italic { Style::Italic } else { Style::Normal };
    match shape.face_metrics(fonts, style.font_size, scale_factor, weight, css_style) {
        Some(m) => m.cell_height() / scale_factor,
        None => style.font_size * 1.3,
    }
}

/// Measure the intrinsic size of `style`'s text in logical pixels (width via
/// the shaper's per-glyph advances, height via the face line height).
pub fn measure_text(
    fonts: &mut FontSystem,
    shape: &mut TextShapeCache,
    style: &TextStyle,
    scale_factor: f32,
) -> Vec2 {
    let weight = Weight(style.weight);
    let css_style = if style.italic { Style::Italic } else { Style::Normal };
    let w = term_gpu::measure_label_width(
        fonts,
        shape,
        &style.text,
        style.font_size,
        scale_factor,
        weight,
        css_style,
    );
    let h = line_height(fonts, shape, style, scale_factor);
    Vec2::new(w, h)
}

/// MEASURE pass (constraints down, sizes up). Computes and stores each node's
/// `measured` size and returns it. Index-based + per-child re-borrow (§14).
pub fn measure(
    tree: &mut RetainedTree,
    id: NodeId,
    constraint: SizeConstraint,
    fonts: &mut FontSystem,
    shape: &mut TextShapeCache,
    scale_factor: f32,
) -> Vec2 {
    // Copy out the small per-node data, releasing the tree borrow before any
    // recursion.
    let kind = tree.node(id).kind.clone();
    let size = match kind {
        NodeKind::Text(style) => constraint.apply(measure_text(fonts, shape, &style, scale_factor)),
        NodeKind::Spacer(sizing) => measure_spacer(sizing, constraint),
        NodeKind::Block(style) => {
            // Block wraps one child; size = child + padding, clamped.
            let pad = style.padding.total();
            let inner_max = (constraint.max - pad).max(Vec2::ZERO);
            let child_constraint = SizeConstraint::loose(inner_max);
            let children = tree.take_children(id);
            let mut inner = Vec2::ZERO;
            if let Some(&child) = children.first() {
                inner = measure(tree, child, child_constraint, fonts, shape, scale_factor);
            }
            tree.restore_children(id, children);
            let border = style.border_width * 2.0;
            constraint.apply(inner + pad + Vec2::splat(border))
        }
        NodeKind::Stack(stack) => measure_stack(
            tree, id, &stack, constraint, fonts, shape, scale_factor,
        ),
    };
    tree.node_mut(id).measured = size;
    size
}

fn measure_spacer(_sizing: Sizing, _constraint: SizeConstraint) -> Vec2 {
    // A spacer has zero intrinsic size. Its extent comes entirely from the
    // parent stack's `child_sizing` (a `Fixed` gap or a `Flex`/`Fill` share),
    // which the stack forces into the spacer's `measured` during its own pass.
    Vec2::ZERO
}

#[allow(clippy::too_many_arguments)]
fn measure_stack(
    tree: &mut RetainedTree,
    id: NodeId,
    stack: &crate::arena::StackStyle,
    constraint: SizeConstraint,
    fonts: &mut FontSystem,
    shape: &mut TextShapeCache,
    scale_factor: f32,
) -> Vec2 {
    let axis = stack.axis;
    let pad = stack.padding.total();
    let inner_max = (constraint.max - pad).max(Vec2::ZERO);

    let children = tree.take_children(id);
    let n = children.len();

    // warpui's hard invariant (a caller PRECONDITION, not a recoverable case):
    // a stack containing a flexible child must NOT be measured under an infinite
    // main-axis constraint — a flex child would want infinite space. Callers
    // (the scroll/overlay layers) must impose a finite main-axis max on any
    // flex-bearing stack. This is a `debug_assert` rather than a release clamp
    // on purpose: silently clamping would mask the caller bug the design treats
    // as a hard invariant (§5). Debug builds (incl. the test suite) panic, so
    // the precondition is enforced where it matters; release builds trust the
    // caller. `r4_reconcile_identity::flex_on_infinite_main_axis_panics` proves
    // the guard fires.
    let has_flex = (0..n).any(|i| tree.node(id).child_sizing(i).flex_weight().is_some());
    if has_flex {
        debug_assert!(
            inner_max.dot(axis_unit(axis)).is_finite(),
            "term_ui: Flex with a flexible child on an infinite main-axis constraint"
        );
    }

    // Pass 1: measure non-flex children at their intrinsic size; sum the main
    // extent of fixed/auto children and the gaps, track max cross extent.
    let gap_total = if n > 0 { stack.gap * (n as f32 - 1.0) } else { 0.0 };
    let mut fixed_main = 0.0_f32;
    let mut max_cross = 0.0_f32;
    let mut flex_weight_sum = 0.0_f32;

    // Child constraint along the cross axis is the inner max; main axis loose.
    for (i, &child) in children.iter().enumerate() {
        let sizing = tree.node(id).child_sizing(i);
        match sizing {
            Sizing::Fixed(px) => {
                let cc = child_constraint(axis, px, inner_max);
                let sz = measure(tree, child, cc, fonts, shape, scale_factor);
                // Force the child's main extent to the fixed value (place reads
                // `measured`); keep its intrinsic cross extent.
                let forced = axis.pack(px, axis.minor(sz));
                tree.node_mut(child).measured = forced;
                fixed_main += px;
                max_cross = max_cross.max(axis.minor(sz));
            }
            Sizing::Flex(w) => flex_weight_sum += w.max(0.0),
            Sizing::Fill => flex_weight_sum += 1.0,
            Sizing::Auto => {
                let cc = SizeConstraint::loose(axis.pack(axis.major(inner_max), axis.minor(inner_max)));
                let sz = measure(tree, child, cc, fonts, shape, scale_factor);
                fixed_main += axis.major(sz);
                max_cross = max_cross.max(axis.minor(sz));
            }
        }
    }

    // Pass 2: distribute leftover main space to flex children and measure them
    // (so their cross extent participates in max_cross and their `measured`
    // main size is the assigned share — `place` reuses it).
    let leftover = (axis.major(inner_max) - fixed_main - gap_total).max(0.0);
    if flex_weight_sum > 0.0 {
        for (i, &child) in children.iter().enumerate() {
            let sizing = tree.node(id).child_sizing(i);
            if let Some(w) = sizing.flex_weight() {
                let share = leftover * (w / flex_weight_sum);
                let cc = child_constraint(axis, share, inner_max);
                let sz = measure(tree, child, cc, fonts, shape, scale_factor);
                // Force the main extent to the assigned share (flex children
                // fill their slot even if their intrinsic size is smaller).
                let forced = axis.pack(share, axis.minor(sz));
                tree.node_mut(child).measured = forced;
                max_cross = max_cross.max(axis.minor(sz));
            }
        }
    }

    tree.restore_children(id, children);

    let used_main = if flex_weight_sum > 0.0 {
        // Flex children consume the leftover, so the stack fills its main max.
        axis.major(inner_max)
    } else {
        fixed_main + gap_total
    };
    let content = axis.pack(used_main, max_cross);
    constraint.apply(content + pad)
}

fn axis_unit(axis: Axis) -> Vec2 {
    match axis {
        Axis::Horizontal => Vec2::new(1.0, 0.0),
        Axis::Vertical => Vec2::new(0.0, 1.0),
    }
}

/// Child constraint: tight on the main axis at `main_extent`, loose on the
/// cross axis up to the parent's inner cross max.
fn child_constraint(axis: Axis, main_extent: f32, inner_max: Vec2) -> SizeConstraint {
    let cross_max = axis.minor(inner_max);
    SizeConstraint::new(
        axis.pack(main_extent, 0.0),
        axis.pack(main_extent, cross_max),
    )
}

/// PLACE pass (positions down). Assigns each node's absolute `bounds.origin`
/// from its parent's distribution. Tree-only; index-based + per-child re-borrow.
pub fn place(tree: &mut RetainedTree, id: NodeId, origin: Vec2) {
    let measured = tree.node(id).measured;
    tree.node_mut(id).bounds = Bounds::new(origin, measured);

    let kind = tree.node(id).kind.clone();
    match kind {
        NodeKind::Text(_) | NodeKind::Spacer(_) => {}
        NodeKind::Block(style) => {
            // A Block stretches its single child to its inner content box. When
            // a parent sized the Block larger than its child (a Fixed/Stretch
            // slot — e.g. a full-width chrome bar wrapping a left-aligned row),
            // the child fills the whole area, so a child stack's own
            // CrossAxis::Stretch reaches the Block's edges. When the Block is
            // sized to its child (the default), `inner` equals the child's own
            // measured size, so this is a no-op.
            let inner = (measured - style.padding.total() - Vec2::splat(style.border_width * 2.0))
                .max(Vec2::ZERO);
            let children = tree.take_children(id);
            if let Some(&child) = children.first() {
                let inner_origin = origin
                    + style.padding.top_left()
                    + Vec2::splat(style.border_width);
                tree.node_mut(child).measured = inner;
                place(tree, child, inner_origin);
            }
            tree.restore_children(id, children);
        }
        NodeKind::Stack(stack) => place_stack(tree, id, &stack, origin, measured),
    }
}

/// PLACE a self-sized subtree CENTERED within `viewport` (logical px). Reads the
/// node's `measured` size (set by a prior [`measure`] under a LOOSE viewport
/// constraint — so the subtree sized to its intrinsic content rather than being
/// clamped to fill the viewport) and places it at the centered origin. The
/// origin is clamped to `>= 0`, so a subtree larger than the viewport pins to
/// the top-left instead of spilling off the top/left edge. This is the overlay
/// centering lever (popups); the plain [`place`] takes an explicit origin.
pub fn place_centered(tree: &mut RetainedTree, id: NodeId, viewport: Vec2) {
    let measured = tree.node(id).measured;
    let origin = ((viewport - measured) * 0.5).max(Vec2::ZERO);
    place(tree, id, origin);
}

fn place_stack(
    tree: &mut RetainedTree,
    id: NodeId,
    stack: &crate::arena::StackStyle,
    origin: Vec2,
    measured: Vec2,
) {
    let axis = stack.axis;
    let pad = stack.padding;
    let inner_origin = origin + pad.top_left();
    let inner_size = (measured - pad.total()).max(Vec2::ZERO);

    let children = tree.take_children(id);
    let n = children.len();
    if n == 0 {
        tree.restore_children(id, children);
        return;
    }

    // Sum children's main extents (their `measured` main size — flex children
    // already had their share forced in during measure) + gaps.
    let mut children_main = 0.0_f32;
    for &child in &children {
        children_main += axis.major(tree.node(child).measured);
    }
    let gap = stack.gap;
    let gap_total = gap * (n as f32 - 1.0);
    let content_main = children_main + gap_total;
    let free = (axis.major(inner_size) - content_main).max(0.0);

    // Main-axis leading offset + inter-child spacing.
    let (mut cursor, extra_gap) = match stack.main {
        MainAxis::Start => (0.0, 0.0),
        MainAxis::Center => (free * 0.5, 0.0),
        MainAxis::End => (free, 0.0),
        MainAxis::SpaceBetween => {
            if n > 1 {
                (0.0, free / (n as f32 - 1.0))
            } else {
                (free * 0.5, 0.0)
            }
        }
    };

    for &child in &children {
        let child_measured = tree.node(child).measured;
        let child_main = axis.major(child_measured);
        let child_cross = axis.minor(child_measured);

        // Cross-axis alignment within the inner cross extent.
        let cross_extent = axis.minor(inner_size);
        let cross_off = match stack.cross {
            CrossAxis::Start => 0.0,
            CrossAxis::Center => (cross_extent - child_cross) * 0.5,
            CrossAxis::End => cross_extent - child_cross,
            CrossAxis::Stretch => 0.0,
        };

        // Stretch: override the child's cross extent to fill.
        if stack.cross == CrossAxis::Stretch {
            let stretched = axis.pack(child_main, cross_extent);
            tree.node_mut(child).measured = stretched;
        }

        let child_origin = inner_origin + axis.pack(cursor, cross_off);
        place(tree, child, child_origin);

        cursor += child_main + gap + extra_gap;
    }

    tree.restore_children(id, children);
}
