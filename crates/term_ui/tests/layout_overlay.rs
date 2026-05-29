//! `term_ui::place_centered` — the overlay centering lever for popups (E.7.0).
//! A self-sized subtree, measured under a LOOSE viewport constraint, places at
//! the centered origin; a subtree larger than the viewport pins to the
//! top-left (origin clamped to >= 0) rather than spilling off-screen. Driven
//! headlessly through the real measure pass (a `FontSystem` shapes the text);
//! no GPU, no window.

use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache};
use term_ui::{
    build_root, measure, place_centered, Block, BlockStyle, Insets, NodeId, RetainedTree,
    SizeConstraint, Text,
};

fn box_view() -> Block {
    Block::new(
        BlockStyle {
            background: [0.1, 0.1, 0.12, 1.0],
            border_color: [0.3, 0.3, 0.35, 1.0],
            border_width: 1.0,
            padding: Insets::all(12.0),
            shadow: None,
        },
        Text::new("a popup box", 13.0, [0.9, 0.9, 0.9, 1.0]),
    )
}

/// Build the box, measure it under `loose(measure_max)` (so it sizes to its
/// intrinsic content), then center it in `viewport`.
fn laid_out(viewport: Vec2, measure_max: Vec2) -> (RetainedTree, NodeId) {
    let mut tree = RetainedTree::new();
    let mut fonts = FontSystem::new();
    let mut shape = TextShapeCache::with_family(FontFamily::SansSerif);
    let view = box_view();
    let root = build_root(&mut tree, &view);
    measure(&mut tree, root, SizeConstraint::loose(measure_max), &mut fonts, &mut shape, 1.0);
    place_centered(&mut tree, root, viewport);
    (tree, root)
}

#[test]
fn centers_a_self_sized_box_on_both_axes() {
    let viewport = Vec2::new(900.0, 400.0);
    let (tree, root) = laid_out(viewport, viewport);
    let b = tree.node(root).bounds;
    let measured = tree.node(root).measured;
    // The box sized to its content — smaller than the window on both axes...
    assert!(
        measured.x < viewport.x && measured.y < viewport.y,
        "box should be smaller than viewport, got {measured:?}"
    );
    // ...and sits at the centered origin.
    let expected = (viewport - measured) * 0.5;
    assert!(
        (b.origin.x - expected.x).abs() < 0.5,
        "centered x: got {}, want {}",
        b.origin.x,
        expected.x
    );
    assert!(
        (b.origin.y - expected.y).abs() < 0.5,
        "centered y: got {}, want {}",
        b.origin.y,
        expected.y
    );
}

#[test]
fn pins_top_left_when_box_overflows_viewport() {
    // Measure under a generous max so the box sizes to its content, then center
    // it in a viewport SMALLER than the box on both axes: the origin clamps to 0
    // instead of going negative (which would place the box partly off-screen).
    let (tree, root) = laid_out(Vec2::new(10.0, 10.0), Vec2::new(1000.0, 1000.0));
    let b = tree.node(root).bounds;
    let measured = tree.node(root).measured;
    assert!(
        measured.x > 10.0 && measured.y > 10.0,
        "box should overflow the tiny viewport, got {measured:?}"
    );
    assert_eq!(b.origin.x, 0.0, "overflowing x pins to the left edge");
    assert_eq!(b.origin.y, 0.0, "overflowing y pins to the top edge");
}
