//! `ui::chrome_labels::chrome_view` — the full-window chrome composition
//! (E.6): an opaque header bar pinned to the top, a transparent fill, an
//! opaque footer bar pinned to the bottom. Pins the layout (positions + full
//! width) headlessly via measure/place; the visual result is verified live
//! once it's wired into the coordinator (E.6b).

use anyclaude::ui::chrome_labels::{
    chrome_view, footer_segments, header_segments, session_widget_id,
};
use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache};
use term_ui::{build_root, measure, place, RetainedTree, SizeConstraint};

#[test]
fn header_pins_top_footer_pins_bottom_both_full_width() {
    let header = header_segments("anthropic", Some("opus"), None, 3, 42, "sid-1", false);
    let (left, right) = footer_segments("0.5.0");
    let (header_h, footer_h) = (30.0, 28.0);
    let view = chrome_view(&header, &left, &right, 14.0, header_h, footer_h, 12.0);

    let (w, h) = (900.0, 400.0);
    let mut tree = RetainedTree::new();
    let mut fonts = FontSystem::new();
    let mut shape = TextShapeCache::with_family(FontFamily::SansSerif);
    let root = build_root(&mut tree, &view);
    measure(
        &mut tree,
        root,
        SizeConstraint::tight(Vec2::new(w, h)),
        &mut fonts,
        &mut shape,
        1.0,
    );
    place(&mut tree, root, Vec2::ZERO);

    // root VStack → [header Block (Fixed), spacer (Fill), footer Block (Fixed)].
    let kids = tree.node(root).children.clone();
    assert_eq!(kids.len(), 3, "header bar, fill, footer bar");

    let header_b = tree.node(kids[0]).bounds;
    assert!(header_b.origin.y.abs() < 0.5, "header at top, y={}", header_b.origin.y);
    assert!((header_b.size.y - header_h).abs() < 0.5, "header height");
    assert!((header_b.size.x - w).abs() < 0.5, "header full width (bg covers edge-to-edge)");

    let footer_b = tree.node(kids[2]).bounds;
    assert!(
        (footer_b.origin.y - (h - footer_h)).abs() < 0.5,
        "footer at bottom, y={}",
        footer_b.origin.y
    );
    assert!((footer_b.size.y - footer_h).abs() < 0.5, "footer height");
    assert!((footer_b.size.x - w).abs() < 0.5, "footer full width");

    // Regression guard: the header's 1px fence must reach the window edge even
    // though it's nested header-Block → header_bar VStack → fence. (The bg
    // Block must stretch its child so the bar's CrossAxis::Stretch fence is
    // full-width, not just text-width.)
    let header_bar = tree.node(kids[0]).children[0]; // the VStack inside the bg Block
    let fence = *tree.node(header_bar).children.last().unwrap();
    let fence_b = tree.node(fence).bounds;
    assert!(
        (fence_b.size.x - w).abs() < 0.5,
        "header fence spans full width through the bg Block, got {}",
        fence_b.size.x
    );
}

#[test]
fn session_run_is_hit_testable_in_the_header() {
    let header_h = 30.0;
    let header = header_segments("anthropic", Some("opus"), None, 3, 42, "sid-abc", false);
    let (left, right) = footer_segments("0.5.0");
    let view = chrome_view(&header, &left, &right, 14.0, header_h, 28.0, 12.0);

    let mut tree = RetainedTree::new();
    let mut fonts = FontSystem::new();
    let mut shape = TextShapeCache::with_family(FontFamily::SansSerif);
    let root = build_root(&mut tree, &view);
    measure(
        &mut tree,
        root,
        SizeConstraint::tight(Vec2::new(900.0, 400.0)),
        &mut fonts,
        &mut shape,
        1.0,
    );
    place(&mut tree, root, Vec2::ZERO);

    // The session run is tagged so the coordinator can resolve its bounds.
    let nid = tree
        .resolve_widget(session_widget_id())
        .expect("session run resolves to a node");
    let b = tree.node(nid).bounds;
    assert!(b.origin.y < header_h, "session label is in the header band, y={}", b.origin.y);
    assert!(b.size.x > 0.0, "session label has a clickable width");
}
