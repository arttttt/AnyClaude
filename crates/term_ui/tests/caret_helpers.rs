//! Coverage for the two new caret helpers (`caret_x` / `byte_at_x`, §8, R9)
//! over a REAL `term_gpu::ShapedLine`. These are the only term_ui-owned
//! additions to the text surface, and without a consumer they were unverified.
//!
//! We assert the directional invariants the helpers document rather than exact
//! pixel values (which depend on the resolved system font):
//! - `caret_x(0)` is the line's left edge (0.0).
//! - `caret_x` is monotonically non-decreasing as the byte index advances over
//!   cluster boundaries.
//! - `caret_x(past-end)` equals the full logical line width.
//! - `byte_at_x` returns cluster boundaries only (never a mid-cluster byte) and
//!   round-trips at glyph left edges: `byte_at_x(caret_x(start)) == start`.
//! - A click far left maps to the first glyph's start; far right to the last
//!   glyph's end.

use term_gpu::{FontFamily, FontSystem, Style, TextShapeCache, Weight};
use term_ui::{byte_at_x, caret_x};

const FS: f32 = 18.0;
const SF: f32 = 2.0;

/// Shape `text` and return the first `ShapedLine` cloned out. ASCII text shapes
/// to a single line; empty text may produce no lines, in which case we hand back
/// an empty line (exactly what the helpers' empty-line path expects).
fn shape_line(text: &str) -> term_gpu::ShapedLine {
    let mut fonts = FontSystem::new();
    let mut cache = TextShapeCache::with_family(FontFamily::SansSerif);
    let shaped = cache.shape(&mut fonts, text, FS, SF, None, Weight(400), Style::Normal);
    match shaped.lines.first() {
        Some(line) => line.clone(),
        None => term_gpu::ShapedLine { glyphs: Vec::new(), line_y: 0.0 },
    }
}

#[test]
fn caret_x_starts_at_left_edge() {
    let line = shape_line("hello");
    assert_eq!(caret_x(&line, 0, SF), 0.0, "caret at byte 0 is the left edge");
}

#[test]
fn caret_x_is_monotonic_and_ends_at_line_width() {
    let text = "hello world";
    let line = shape_line(text);

    let mut prev = -1.0_f32;
    for byte in 0..=text.len() {
        let x = caret_x(&line, byte, SF);
        assert!(
            x >= prev,
            "caret_x must be non-decreasing: byte {byte} gave {x} < {prev}"
        );
        prev = x;
    }

    // Past the end maps to the full logical line width = (last.x + last.w) / sf.
    let last = line.glyphs.last().expect("non-empty");
    let expected_width = (last.x + last.w) / SF;
    assert_eq!(
        caret_x(&line, text.len(), SF),
        expected_width,
        "caret past end is the line width"
    );
}

#[test]
fn byte_at_x_round_trips_glyph_left_edges() {
    let text = "abcde";
    let line = shape_line(text);

    // Each glyph's left edge (in logical px) should map back to that glyph's
    // start byte. ASCII => one glyph per byte, so start advances 0,1,2,...
    for g in &line.glyphs {
        let logical_left = g.x / SF;
        let byte = byte_at_x(&line, logical_left, SF);
        assert_eq!(byte, g.start, "click at a glyph's left edge maps to its start byte");
    }
}

#[test]
fn byte_at_x_clamps_to_first_and_last() {
    let text = "xyz";
    let line = shape_line(text);
    let first = line.glyphs.first().expect("non-empty");
    let last = line.glyphs.last().expect("non-empty");

    assert_eq!(
        byte_at_x(&line, -1000.0, SF),
        first.start,
        "far-left click maps to the first glyph's start"
    );
    assert_eq!(
        byte_at_x(&line, 1_000_000.0, SF),
        last.end,
        "far-right click maps to the last glyph's end"
    );
}

#[test]
fn empty_line_is_origin() {
    let line = shape_line("");
    assert_eq!(caret_x(&line, 0, SF), 0.0);
    assert_eq!(caret_x(&line, 5, SF), 0.0);
    assert_eq!(byte_at_x(&line, 50.0, SF), 0);
}
