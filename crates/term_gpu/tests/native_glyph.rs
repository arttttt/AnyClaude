//! Native rect-painted glyphs (block chars + the pause symbol).
//!
//! `paint_block_char` returns solid rects instead of routing through the
//! font shaper. The pause glyph ⏸ (U+23F8) has no monospace coverage, so it
//! is painted as two vertical bars — these tests pin that geometry.

use term_gpu::{paint_block_char, RectInstance};

/// Paint a single char into a 10×20 cell at the origin and return its rects.
fn paint(ch: char) -> (bool, Vec<RectInstance>) {
    let mut rects = Vec::new();
    let handled = paint_block_char(ch, 0.0, 0.0, 10.0, 20.0, [1.0, 1.0, 1.0, 1.0], &mut rects);
    (handled, rects)
}

#[test]
fn pause_paints_two_vertical_bars() {
    let (handled, rects) = paint('\u{23F8}'); // ⏸
    assert!(handled, "pause must be handled natively, not shaped");
    assert_eq!(rects.len(), 2, "pause is exactly two bars");

    let (a, b) = (&rects[0], &rects[1]);
    // Both bars are taller than they are wide (vertical), same size, same y.
    for bar in [a, b] {
        assert!(bar.size[1] > bar.size[0], "each bar is vertical: {bar:?}");
    }
    assert_eq!(a.size, b.size, "bars are identical in size");
    assert_eq!(a.pos[1], b.pos[1], "bars share the same top edge");

    // The two bars don't overlap and there is a gap between them.
    let (left, right) = if a.pos[0] <= b.pos[0] { (a, b) } else { (b, a) };
    let left_end = left.pos[0] + left.size[0];
    assert!(left_end < right.pos[0], "a visible gap separates the two bars");

    // The pair stays inside the cell [0,10] × [0,20].
    assert!(left.pos[0] >= 0.0);
    assert!(right.pos[0] + right.size[0] <= 10.0);
    assert!(a.pos[1] >= 0.0 && a.pos[1] + a.size[1] <= 20.0);

    // The pair is roughly horizontally centered in the cell.
    let pair_mid = (left.pos[0] + right.pos[0] + right.size[0]) / 2.0;
    assert!((pair_mid - 5.0).abs() < 0.5, "pair centered in cell, mid={pair_mid}");
}

#[test]
fn non_block_char_is_not_handled() {
    // A plain letter must fall through to the font path.
    let (handled, rects) = paint('A');
    assert!(!handled);
    assert!(rects.is_empty());
}

#[test]
fn full_block_fills_the_cell() {
    // Sanity check the existing block path still works alongside the new arm.
    let (handled, rects) = paint('\u{2588}'); // █
    assert!(handled);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0].size, [10.0, 20.0]);
}
