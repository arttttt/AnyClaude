//! Variable-width text labels — for chrome (header / footer) and any
//! UI text outside the terminal grid.
//!
//! `push_label` shapes a single-line text run through `TextShapeCache`,
//! rasterises each glyph through the atlas, and appends the resulting
//! `GlyphInstance`s to the caller's buffer. Returns the rightmost x
//! coordinate (in logical pixels) so callers can chain labels.
//!
//! `measure_label_width` returns the same x-advance without emitting
//! any glyphs — useful for layout (e.g. deciding header column widths
//! before rendering).
//!
//! Labels are positioned by *baseline*, not top — that's the natural
//! anchor for shaped text and matches what `LayoutGlyph::physical`
//! expects.

use cosmic_text::{FontSystem, Style, SwashCache, Weight};

use crate::{rasterize_glyph, GlyphAtlas, GlyphInstance, TextShapeCache};

/// Shape `text`, rasterise each glyph through `atlas`, and append the
/// produced `GlyphInstance`s to `glyphs`. Returns the x-coordinate
/// (logical pixels) just past the last glyph so the caller can chain
/// the next label without re-measuring.
///
/// `baseline_y_logical` is the Y of the text baseline, not the top —
/// callers usually compute it as `top + line_height * 0.75` or pull
/// the value from a cached `CellMetrics`-style layout.
#[allow(clippy::too_many_arguments)]
pub fn push_label(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    shape_cache: &mut TextShapeCache,
    glyphs: &mut Vec<GlyphInstance>,
    text: &str,
    origin_x_logical: f32,
    baseline_y_logical: f32,
    font_size: f32,
    scale_factor: f32,
    weight: Weight,
    style: Style,
    color: [f32; 4],
) -> f32 {
    let sf = scale_factor;
    let origin_x_phys = origin_x_logical * sf;
    let baseline_y_phys = (baseline_y_logical * sf).round();
    let shaped = shape_cache.shape(font_system, text, font_size, sf, None, weight, style);
    let mut max_right_phys = origin_x_phys;
    for line in &shaped.lines {
        for glyph in &line.glyphs {
            let physical = glyph.physical((origin_x_phys, baseline_y_phys), 1.0);
            let Some(placed) = atlas.get_or_insert(physical.cache_key, || {
                rasterize_glyph(font_system, swash_cache, physical.cache_key)
            }) else {
                continue;
            };
            let pos_x = (physical.x as f32 + placed.offset_x) / sf;
            let pos_y = (physical.y as f32 - placed.offset_y) / sf;
            glyphs.push(GlyphInstance {
                pos: [pos_x, pos_y],
                size: [placed.width / sf, placed.height / sf],
                uv_min: placed.uv_min,
                uv_max: placed.uv_max,
                color,
                layer: placed.layer,
            });
            let glyph_right_phys = origin_x_phys + glyph.x + glyph.w;
            if glyph_right_phys > max_right_phys {
                max_right_phys = glyph_right_phys;
            }
        }
    }
    max_right_phys / sf
}

/// Measure the x-advance of `text` without emitting any glyphs.
/// Returns the width in logical pixels.
#[allow(clippy::too_many_arguments)]
pub fn measure_label_width(
    font_system: &mut FontSystem,
    shape_cache: &mut TextShapeCache,
    text: &str,
    font_size: f32,
    scale_factor: f32,
    weight: Weight,
    style: Style,
) -> f32 {
    let shaped = shape_cache.shape(
        font_system,
        text,
        font_size,
        scale_factor,
        None,
        weight,
        style,
    );
    let mut max_right_phys: f32 = 0.0;
    for line in &shaped.lines {
        for glyph in &line.glyphs {
            let right = glyph.x + glyph.w;
            if right > max_right_phys {
                max_right_phys = right;
            }
        }
    }
    max_right_phys / scale_factor
}
