//! Caret ↔ pixel mapping over a `term_gpu::ShapedLine` (design §8, R9).
//!
//! `measure_label_width` returns only a total width, which is insufficient for
//! caret placement. A text field shapes its value once into a `ShapedLine`
//! (whose `glyphs: Vec<LayoutGlyph>` carry per-glyph `x`/`w` and the cluster's
//! byte range `start..end`) and these two pure helpers map both directions.
//!
//! These live in term_ui (not term_gpu) per the §12 green-build caveat: the
//! literal `label.rs` MOVE is deferred to a later phase to keep the live
//! `src/ui/gpu/*` consumers of `term_gpu::push_label` compiling, but the TWO
//! NEW helpers belong to term_ui from the start and are built on the same
//! `ShapedLine`.
//!
//! Coordinate convention: `ShapedLine` glyph `x`/`w` are PHYSICAL pixels
//! (cosmic-text shapes at `font_size * scale_factor`). Both helpers take a
//! `scale_factor` and work in **logical** pixels (matching
//! `measure_label_width`, which divides its result by the scale factor).
//!
//! Byte indices are byte offsets into the original shaped string; per the §8
//! grapheme policy a field snaps caret movement to cluster boundaries, and
//! `byte_at_x` returns a cluster boundary, never a mid-cluster byte.

use term_gpu::ShapedLine;

/// Caret byte index → logical pixel X (to draw the caret rect).
///
/// Returns the left edge (`x`) of the glyph whose cluster starts at `byte`.
/// `byte` past the end maps to the right edge of the last glyph (line width);
/// an empty line maps to 0.0. A `byte` landing inside a multi-byte cluster
/// snaps to that cluster's left edge.
pub fn caret_x(shaped: &ShapedLine, byte: usize, scale_factor: f32) -> f32 {
    let sf = scale_factor.max(f32::MIN_POSITIVE);
    if shaped.glyphs.is_empty() {
        return 0.0;
    }
    // Glyphs are in visual (left-to-right) order for our LTR chrome; find the
    // first glyph whose cluster covers or starts at `byte`.
    for g in &shaped.glyphs {
        if byte <= g.start {
            return g.x / sf;
        }
        if byte < g.end {
            // Inside this cluster — snap to its left edge.
            return g.x / sf;
        }
    }
    // Past the last glyph: right edge of the line.
    let last = shaped.glyphs.last().expect("non-empty checked above");
    (last.x + last.w) / sf
}

/// Click X (logical pixels) → caret byte index (to place caret / start
/// selection).
///
/// Returns the cluster boundary nearest the click: the `start` of the glyph
/// the click falls in if the click is in its left half, else its `end`. A
/// click before the first glyph returns the first glyph's `start`; after the
/// last returns the last glyph's `end`. An empty line returns 0.
pub fn byte_at_x(shaped: &ShapedLine, x: f32, scale_factor: f32) -> usize {
    let sf = scale_factor.max(f32::MIN_POSITIVE);
    let x_phys = x * sf;
    let Some(first) = shaped.glyphs.first() else {
        return 0;
    };
    if x_phys <= first.x {
        return first.start;
    }
    for g in &shaped.glyphs {
        let left = g.x;
        let right = g.x + g.w;
        if x_phys < right {
            let mid = left + g.w * 0.5;
            return if x_phys < mid { g.start } else { g.end };
        }
    }
    let last = shaped.glyphs.last().expect("non-empty checked above");
    last.end
}
