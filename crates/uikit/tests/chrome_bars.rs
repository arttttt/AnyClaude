//! Layout contract for the chrome bars: the bits a consumer relies on —
//! flush-right footer version, full-width fence on the correct edge, and the
//! N-segments → N-1-separators interleaving. Driven headlessly through the real
//! term_ui measure/place passes (a `FontSystem` shapes the text); no GPU, no
//! window. Reconcile *identity* is already gated by term_ui's R4 suite — this
//! file only pins the geometry uikit adds on top.

use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache};
use term_ui::{build_root, measure, place, NodeId, RetainedTree, SizeConstraint, Stack};
use uikit::{footer_bar, header_bar, Segment};

const DIM: [f32; 4] = [0.55, 0.55, 0.55, 1.0];
const FENCE: [f32; 4] = [0.25, 0.25, 0.27, 1.0];
const FONT: f32 = 12.0;
const PAD: f32 = 8.0;

/// Build `view`, then run measure (tight `w`×`h`) + place at the origin —
/// exactly how a coordinator lays out a strip pinned to a fixed region size.
fn layout(view: &Stack, w: f32, h: f32) -> (RetainedTree, NodeId) {
    let mut tree = RetainedTree::new();
    let mut fonts = FontSystem::new();
    let mut shape = TextShapeCache::with_family(FontFamily::SansSerif);
    let root = build_root(&mut tree, view);
    measure(
        &mut tree,
        root,
        SizeConstraint::tight(Vec2::new(w, h)),
        &mut fonts,
        &mut shape,
        1.0,
    );
    place(&mut tree, root, Vec2::ZERO);
    (tree, root)
}

#[test]
fn footer_version_is_flush_right() {
    let w = 800.0;
    let view = footer_bar(
        &[Segment::new(" Cmd+B: Switch │ Cmd+Q: Quit", DIM)],
        &[Segment::new("v0.5.0 ", DIM)],
        FONT,
        FENCE,
    );
    let (tree, root) = layout(&view, w, 22.0);
    // root VStack -> [fence, row]; row -> [hints, spacer, version].
    let row = tree.node(root).children[1];
    let version = *tree.node(row).children.last().unwrap();
    let b = tree.node(version).bounds;
    assert!(
        (b.right() - w).abs() < 1.0,
        "version right edge = {}, want ≈ {w} (flush-right)",
        b.right()
    );
}

#[test]
fn header_fence_spans_full_width_at_bottom() {
    let (w, h) = (800.0, 24.0);
    let view = header_bar(
        &[
            Segment::new("backend: anthropic", DIM),
            Segment::new("Session: abc123", DIM),
        ],
        " │ ",
        DIM,
        FONT,
        PAD,
        FENCE,
    );
    let (tree, root) = layout(&view, w, h);
    // root VStack -> [row (Fill), fence (Fixed 1)].
    let fence = tree.node(root).children[1];
    let b = tree.node(fence).bounds;
    assert!(b.origin.x.abs() < 0.5, "fence x = {}", b.origin.x);
    assert!((b.size.x - w).abs() < 0.5, "fence width = {}, want ≈ {w}", b.size.x);
    assert!((b.size.y - 1.0).abs() < 0.5, "fence height = {}, want 1", b.size.y);
    assert!(
        (b.origin.y - (h - 1.0)).abs() < 0.5,
        "fence y = {}, want ≈ {} (bottom edge)",
        b.origin.y,
        h - 1.0
    );
}

#[test]
fn footer_fence_spans_full_width_at_top() {
    let w = 640.0;
    let view = footer_bar(
        &[Segment::new("hints", DIM)],
        &[Segment::new("v", DIM)],
        FONT,
        FENCE,
    );
    let (tree, root) = layout(&view, w, 22.0);
    // root VStack -> [fence (Fixed 1), row (Fill)].
    let fence = tree.node(root).children[0];
    let b = tree.node(fence).bounds;
    assert!(b.origin.y.abs() < 0.5, "fence y = {}, want ≈ 0 (top edge)", b.origin.y);
    assert!((b.size.x - w).abs() < 0.5, "fence width = {}, want ≈ {w}", b.size.x);
    assert!((b.size.y - 1.0).abs() < 0.5, "fence height = {}, want 1", b.size.y);
}

#[test]
fn header_interleaves_one_separator_between_segments() {
    let view = header_bar(
        &[
            Segment::new("a", DIM),
            Segment::new("b", DIM),
            Segment::new("c", DIM),
        ],
        " | ",
        DIM,
        FONT,
        PAD,
        FENCE,
    );
    let (tree, root) = layout(&view, 800.0, 24.0);
    let row = tree.node(root).children[0];
    // 3 segments → 3 texts + 2 separators = 5 children.
    assert_eq!(tree.node(row).children.len(), 5);
}
