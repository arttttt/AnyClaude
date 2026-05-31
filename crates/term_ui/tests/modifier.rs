//! Modifier fold: measure adds the box insets; paint honours chain ORDER (the
//! box-model property — `background().padding()` vs `padding().background()`).
//! Headless via `paint_cpu` (RectRecords), so no atlas/GPU.

use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache};
use term_ui::{
    build_root, measure, paint_cpu, place, CpuPaint, Element, Insets, Modify, Modifier,
    RetainedTree, SizeConstraint, Text,
};

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

fn render_cpu<E: Element>(view: E) -> (CpuPaint, Vec2) {
    let mut fonts = FontSystem::new();
    let mut shape = TextShapeCache::with_family(FontFamily::SansSerif);
    let mut tree = RetainedTree::new();
    let root = build_root(&mut tree, &view);
    let size = measure(
        &mut tree,
        root,
        SizeConstraint::loose(Vec2::new(1000.0, 1000.0)),
        &mut fonts,
        &mut shape,
        1.0,
    );
    place(&mut tree, root, Vec2::ZERO);
    let mut cpu = CpuPaint::default();
    paint_cpu(&tree, root, &mut cpu, &mut fonts, &mut shape, 1.0);
    (cpu, size)
}

fn text() -> Text {
    Text::new("Hi", 14.0, [1.0; 4])
}

#[test]
fn padding_adds_to_the_measured_size() {
    let (_, plain) = render_cpu(text());
    let (_, padded) = render_cpu(text().modify(Modifier::new().padding(Insets::all(10.0))));
    assert!((padded.x - plain.x - 20.0).abs() < 0.5, "padded {} vs plain {}", padded.x, plain.x);
    assert!((padded.y - plain.y - 20.0).abs() < 0.5);
}

#[test]
fn margin_and_border_also_inset_the_size() {
    let (_, plain) = render_cpu(text());
    let (_, boxed) = render_cpu(text().modify(
        Modifier::new().margin(Insets::all(5.0)).border(2.0, RED).padding(Insets::all(8.0)),
    ));
    // per side: margin 5 + border 2 + padding 8 = 15 → +30 per axis.
    assert!((boxed.x - plain.x - 30.0).abs() < 0.5);
    assert!((boxed.y - plain.y - 30.0).abs() < 0.5);
}

#[test]
fn paint_honours_chain_order_for_the_background() {
    // background BEFORE padding → bg covers the OUTER (full) bounds at (0,0).
    let (a, a_size) = render_cpu(text().modify(Modifier::new().background(RED).padding(Insets::all(10.0))));
    let bg_a = a.rects.iter().find(|r| r.color == RED).expect("bg A present");
    assert_eq!(bg_a.origin, [0.0, 0.0], "bg before padding spans the full bounds");
    assert!((bg_a.size[0] - a_size.x).abs() < 0.5);

    // padding BEFORE background → bg is INSET to the inner box at (10,10).
    let (b, _) = render_cpu(text().modify(Modifier::new().padding(Insets::all(10.0)).background(RED)));
    let bg_b = b.rects.iter().find(|r| r.color == RED).expect("bg B present");
    assert_eq!(bg_b.origin, [10.0, 10.0], "bg after padding is inset by the padding");
    assert!(
        (bg_a.size[0] - bg_b.size[0] - 20.0).abs() < 0.5,
        "outer bg is wider than the inner bg by the padding"
    );
}
