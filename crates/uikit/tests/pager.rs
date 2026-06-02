//! `uikit::pager` — the horizontal pager. Verifies the two load-bearing bits of
//! the "translate the track" model through the real term_ui measure/place/paint
//! passes (no GPU): page `i` lands at `(i - scroll)·page_w`, and the windowed
//! pages are clipped to the viewport. Layout is driven tight to a viewport rect,
//! exactly as the host (the teammates overlay) drives it.

use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache, NO_CLIP};
use term_ui::{
    build_root, measure, paint_cpu, place, BoxView, CpuPaint, Element, Modifier, Modify, NodeId,
    RetainedTree, SizeConstraint, Text, WidgetId,
};
use uikit::{pager, PagerPalette};

const PAL: PagerPalette = PagerPalette {
    dot_current: [1.0, 1.0, 1.0, 1.0],
    dot_idle: [0.5, 0.5, 0.5, 1.0],
    arrow: [0.7, 0.7, 0.7, 1.0],
};

/// A page is a coloured background wrapping a glyph, so it emits an inspectable
/// `RectRecord` (with the folded clip) in `paint_cpu`.
fn page(color: [f32; 4]) -> BoxView {
    Box::new(Text::new("p", 13.0, [1.0; 4]).modify(Modifier::new().background(color)))
}

fn base() -> WidgetId {
    WidgetId::from_path(&[7])
}

/// Build + lay out a view tight to a `viewport`, returning the tree + root.
fn laid_out<E: Element>(view: E, viewport: Vec2) -> (RetainedTree, NodeId) {
    let mut tree = RetainedTree::new();
    let mut fonts = FontSystem::new();
    let mut shape = TextShapeCache::with_family(FontFamily::SansSerif);
    let root = build_root(&mut tree, &view);
    measure(&mut tree, root, SizeConstraint::tight(viewport), &mut fonts, &mut shape, 1.0);
    place(&mut tree, root, Vec2::ZERO);
    (tree, root)
}

#[test]
fn settled_current_page_sits_at_the_viewport_neighbours_one_over() {
    let r = [1.0, 0.0, 0.0, 1.0];
    let g = [0.0, 1.0, 0.0, 1.0];
    let b = [0.0, 0.0, 1.0, 1.0];
    // Focused on the middle page, settled (scroll == current == 1).
    let view = pager(vec![page(r), page(g), page(b)], 1, 1.0, 200.0, 20.0, 13.0, PAL, base());
    let (tree, root) = laid_out(view, Vec2::new(200.0, 100.0));

    // root vstack → [clip viewport, strip]; viewport → offset-mod → track → pages
    let viewport = tree.node(root).children[0];
    let offset_mod = tree.node(viewport).children[0];
    let track = tree.node(offset_mod).children[0];
    let pages = tree.node(track).children.clone();
    assert_eq!(pages.len(), 3, "current ± 1 window covers all three pages");

    let x = |i: usize| tree.node(pages[i]).bounds.origin.x;
    assert!(x(1).abs() < 0.5, "current page at the viewport (x=0), got {}", x(1));
    assert!((x(0) + 200.0).abs() < 0.5, "prev page one viewport left, got {}", x(0));
    assert!((x(2) - 200.0).abs() < 0.5, "next page one viewport right, got {}", x(2));
}

#[test]
fn mid_slide_offsets_pages_by_the_fraction() {
    // Halfway from page 0 to page 1 → page 0 is half a viewport to the left,
    // page 1 half a viewport to the right.
    let view = pager(
        vec![page([1.0, 0.0, 0.0, 1.0]), page([0.0, 1.0, 0.0, 1.0])],
        0,
        0.5,
        200.0,
        20.0,
        13.0,
        PAL,
        base(),
    );
    let (tree, root) = laid_out(view, Vec2::new(200.0, 100.0));
    let track = {
        let viewport = tree.node(root).children[0];
        let offset_mod = tree.node(viewport).children[0];
        tree.node(offset_mod).children[0]
    };
    let pages = tree.node(track).children.clone();
    let x = |i: usize| tree.node(pages[i]).bounds.origin.x;
    assert!((x(0) + 100.0).abs() < 0.5, "page 0 at -0.5·W, got {}", x(0));
    assert!((x(1) - 100.0).abs() < 0.5, "page 1 at +0.5·W, got {}", x(1));
}

#[test]
fn pages_are_clipped_to_the_page_viewport() {
    let view = pager(
        vec![page([1.0, 0.0, 0.0, 1.0]), page([0.0, 1.0, 0.0, 1.0])],
        0,
        0.0,
        200.0,
        20.0,
        13.0,
        PAL,
        base(),
    );
    let (tree, root) = laid_out(view, Vec2::new(200.0, 100.0));
    let mut fonts = FontSystem::new();
    let mut shape = TextShapeCache::with_family(FontFamily::SansSerif);
    let mut cpu = CpuPaint::default();
    paint_cpu(&tree, root, &mut cpu, &mut fonts, &mut shape, 1.0);

    // The page area is the full width and the height minus the 20px strip → the
    // page backgrounds carry that clip; nothing in the tree is left unclipped.
    let clipped: Vec<_> = cpu.rects.iter().filter(|r| r.clip != NO_CLIP).collect();
    assert_eq!(clipped.len(), 2, "both windowed page backgrounds are clipped");
    for r in clipped {
        assert_eq!(r.clip, [0.0, 0.0, 200.0, 80.0], "clipped to the page viewport");
    }
}
