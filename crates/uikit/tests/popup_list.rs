//! `uikit::popup_list` + `uikit::fixed_row_window` (E.7.0). The list places one
//! highlight `Block` at the selected row (others plain `Text`) and stretches it
//! to the full list width; the window helper slices the visible row range for a
//! long scrolled list. Layout is driven headlessly through the real term_ui
//! measure/place passes; no GPU.

use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache};
use term_ui::{build_root, measure, place, NodeKind, RetainedTree, SizeConstraint};
use uikit::{fixed_row_window, popup_list, Segment};

const DIM: [f32; 4] = [0.55, 0.55, 0.55, 1.0];
const BRIGHT: [f32; 4] = [0.95, 0.95, 0.95, 1.0];
const HL: [f32; 4] = [0.22, 0.30, 0.42, 1.0];

fn rows() -> Vec<Segment> {
    vec![
        Segment::new("first row", DIM),
        Segment::new("second row", BRIGHT),
        Segment::new("third row", DIM),
    ]
}

#[test]
fn selected_row_is_a_highlight_block_others_are_text() {
    let view = popup_list(&rows(), 1, 22.0, HL, 13.0);
    let mut tree = RetainedTree::new();
    let root = build_root(&mut tree, &view);
    let kids = tree.node(root).children.clone();
    assert_eq!(kids.len(), 3, "one child per row");
    assert!(matches!(tree.node(kids[0]).kind, NodeKind::Text(_)), "row 0 is plain text");
    assert!(matches!(tree.node(kids[1]).kind, NodeKind::Block(_)), "row 1 is a highlight block");
    assert!(matches!(tree.node(kids[2]).kind, NodeKind::Text(_)), "row 2 is plain text");
    if let NodeKind::Block(style) = &tree.node(kids[1]).kind {
        assert_eq!(style.background, HL, "highlight bar uses hl_bg");
    }
    // The highlight block wraps exactly the row's text.
    let inner = tree.node(kids[1]).children.clone();
    assert_eq!(inner.len(), 1);
    assert!(matches!(tree.node(inner[0]).kind, NodeKind::Text(_)));
}

#[test]
fn highlight_bar_stretches_to_full_list_width() {
    let view = popup_list(&rows(), 1, 22.0, HL, 13.0);
    let (w, h) = (300.0, 66.0);
    let mut tree = RetainedTree::new();
    let mut fonts = FontSystem::new();
    let mut shape = TextShapeCache::with_family(FontFamily::SansSerif);
    let root = build_root(&mut tree, &view);
    measure(&mut tree, root, SizeConstraint::tight(Vec2::new(w, h)), &mut fonts, &mut shape, 1.0);
    place(&mut tree, root, Vec2::ZERO);
    let hl = tree.node(root).children[1];
    let b = tree.node(hl).bounds;
    assert!((b.size.x - w).abs() < 0.5, "highlight spans full width: got {}, want {w}", b.size.x);
    assert!((b.size.y - 22.0).abs() < 0.5, "highlight is one row tall: got {}", b.size.y);
}

#[test]
fn selection_out_of_range_highlights_nothing() {
    let view = popup_list(&rows(), 99, 22.0, HL, 13.0);
    let mut tree = RetainedTree::new();
    let root = build_root(&mut tree, &view);
    for &k in &tree.node(root).children {
        assert!(
            matches!(tree.node(k).kind, NodeKind::Text(_)),
            "no highlight block when selected is out of range"
        );
    }
}

#[test]
fn fixed_row_window_slices_the_visible_range() {
    // Whole list fits: no windowing, offset ignored.
    assert_eq!(fixed_row_window(0, 5, 14), 0..5);
    assert_eq!(fixed_row_window(3, 5, 14), 0..5);
    // Long list, scrolled to the top.
    assert_eq!(fixed_row_window(0, 100, 14), 0..14);
    // Scrolled into the middle.
    assert_eq!(fixed_row_window(20, 100, 14), 20..34);
    // Scrolled past the end clamps to the last full page.
    assert_eq!(fixed_row_window(999, 100, 14), 86..100);
    // Degenerate inputs yield an empty range (no panic).
    assert!(fixed_row_window(0, 0, 14).is_empty());
    assert!(fixed_row_window(5, 50, 0).is_empty());
}
