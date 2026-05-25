//! Bridge between `cosmic-text` and our glyph atlas.
//!
//! Owns nothing — the caller passes in `&mut FontSystem` and `&mut SwashCache`
//! so they can be shared across whatever owns the long-lived text state
//! (a single `GpuRenderer`, a panel, a `Buffer`, etc.).
//!
//! ## Subpixel positioning
//!
//! `cosmic_text::CacheKey` already includes `x_bin` and `y_bin` of type
//! `SubpixelBin` (4 variants each: `Zero`, `One`, `Two`, `Three`). When the
//! shaper computes `glyph.physical(offset, scale)`, the fractional part of
//! the position bins into one of 16 combinations, and the rasterizer
//! produces the glyph image aligned to that subpixel offset.
//!
//! So **subpixel positioning is automatic for us** — we cache by the full
//! `CacheKey` (which includes both bins) and the right image lands in the
//! atlas. Memory cost is ×16 per glyph variant vs Warp's ×3 (Warp does
//! 3 X-bins and snaps Y in the vertex shader); we accept the extra memory
//! for crisper Y positioning and zero hand-rolled code. See
//! `docs/gpu-terminal-spec.md` §5.6 for the rationale and
//! `memory/gpu-terminal-architecture.md` for the locked-in decision.
//!
//! ## Font fallback
//!
//! cosmic-text's `FontSystem` scans the OS font database on construction
//! (`fontdb` under the hood) and resolves missing glyphs by walking the
//! database — so colour emoji (`Apple Color Emoji`, `Noto Color Emoji`,
//! `Segoe UI Emoji`), CJK, and other scripts work out of the box on a
//! system with the corresponding fonts installed. We do not configure an
//! explicit fallback chain — the OS already provides one, and overriding
//! it tends to make things worse, not better.
//!
//! What we **do** expose is the *primary* family preference via
//! [`FontFamily`] and [`TextShapeCache::with_family`]. Pick `SansSerif` for
//! UI text, `Monospace` for code/terminal cell rendering, or `Named` for a
//! specific face. Cache instances are family-scoped: keep separate
//! `TextShapeCache`s for distinct families to avoid spurious cache misses.

use std::collections::HashMap;

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, LayoutGlyph, Metrics, Shaping, SwashCache,
    SwashContent,
};

/// Primary font family preference for a `TextShapeCache`. Maps onto
/// `cosmic_text::Family`. Glyphs absent from the primary face are resolved
/// from the system font database automatically — emoji, CJK, RTL scripts
/// work without further configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FontFamily {
    SansSerif,
    Serif,
    Monospace,
    Cursive,
    Fantasy,
    Named(String),
}

impl FontFamily {
    fn as_cosmic(&self) -> Family<'_> {
        match self {
            FontFamily::SansSerif => Family::SansSerif,
            FontFamily::Serif => Family::Serif,
            FontFamily::Monospace => Family::Monospace,
            FontFamily::Cursive => Family::Cursive,
            FontFamily::Fantasy => Family::Fantasy,
            FontFamily::Named(name) => Family::Name(name.as_str()),
        }
    }
}

use crate::atlas::{GlyphFormat, RasterizedGlyph};

/// Drop entries from the shape cache after this many consecutive frames
/// without a hit. Cheap to keep entries around (text is small), but unbounded
/// growth becomes a problem when callers shape unique text per row.
const SHAPE_CACHE_MAX_UNUSED_FRAMES: u32 = 60;

/// One layout line of a shaped text run, with the line's baseline Y relative
/// to the text origin. Each `LayoutGlyph` retains the layout-relative `x`/`y`
/// it was placed at; on draw, callers add their own origin and call
/// `LayoutGlyph::physical(...)` to get the final `CacheKey` for atlas lookup.
#[derive(Debug, Clone)]
pub struct ShapedLine {
    pub glyphs: Vec<LayoutGlyph>,
    pub line_y: f32,
}

/// Output of shaping one piece of text — reusable across origins.
/// `line_y` and per-glyph `x`/`y` are in physical pixels (cosmic-text
/// shapes at `font_size * scale_factor`); callers do the final
/// `glyph.physical((origin_physical, origin_physical + line_y), 1.0)` to
/// bin into subpixel cache keys.
#[derive(Debug, Clone)]
pub struct ShapedText {
    pub lines: Vec<ShapedLine>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ShapeKey {
    text: String,
    font_size_bits: u32,
    scale_factor_bits: u32,
    wrap_width_bits: Option<u32>,
}

struct CachedShape {
    text: ShapedText,
    last_used_frame: u32,
}

/// Caches shaped text by `(text, font_size, scale_factor, wrap_width)`.
/// Re-shape is cheap (~µs per call in cosmic-text) but adds up at hundreds
/// of labels per frame; the cache turns those into HashMap hits.
///
/// Entries are evicted by frame counter — call `end_frame()` once per
/// rendered frame so entries unused for `SHAPE_CACHE_MAX_UNUSED_FRAMES`
/// drop out. Same pattern as `GlyphAtlas`.
pub struct TextShapeCache {
    entries: HashMap<ShapeKey, CachedShape>,
    frame: u32,
    family: FontFamily,
}

impl Default for TextShapeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TextShapeCache {
    /// Default cache: `FontFamily::SansSerif`. Good for UI labels.
    pub fn new() -> Self {
        Self::with_family(FontFamily::SansSerif)
    }

    pub fn with_family(family: FontFamily) -> Self {
        Self {
            entries: HashMap::new(),
            frame: 0,
            family,
        }
    }

    pub fn family(&self) -> &FontFamily {
        &self.family
    }

    /// Look up or shape. Sizes are in **logical** pixels; the cache shapes
    /// internally at `font_size * scale_factor` so swash rasterizes at the
    /// display's physical density.
    pub fn shape(
        &mut self,
        font_system: &mut FontSystem,
        text: &str,
        font_size: f32,
        scale_factor: f32,
        wrap_width: Option<f32>,
    ) -> &ShapedText {
        let key = ShapeKey {
            text: text.to_string(),
            font_size_bits: font_size.to_bits(),
            scale_factor_bits: scale_factor.to_bits(),
            wrap_width_bits: wrap_width.map(|w| w.to_bits()),
        };
        let frame = self.frame;
        let family = &self.family;
        let entry = self.entries.entry(key).or_insert_with(|| CachedShape {
            text: shape_text_inline(
                font_system,
                text,
                font_size * scale_factor,
                wrap_width.map(|w| w * scale_factor),
                family,
            ),
            last_used_frame: frame,
        });
        entry.last_used_frame = frame;
        &entry.text
    }

    /// Advance the frame counter and evict entries that have not been
    /// requested in the last `SHAPE_CACHE_MAX_UNUSED_FRAMES` frames.
    pub fn end_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        let now = self.frame;
        self.entries
            .retain(|_, c| now.wrapping_sub(c.last_used_frame) <= SHAPE_CACHE_MAX_UNUSED_FRAMES);
    }
}

fn shape_text_inline(
    font_system: &mut FontSystem,
    text: &str,
    font_size_physical: f32,
    wrap_width_physical: Option<f32>,
    family: &FontFamily,
) -> ShapedText {
    let line_height = font_size_physical * 1.3;
    let metrics = Metrics::new(font_size_physical, line_height);
    let mut buffer = Buffer::new_empty(metrics);
    buffer.set_size(font_system, wrap_width_physical, None);
    let attrs = Attrs::new().family(family.as_cosmic());
    buffer.set_text(font_system, text, &attrs, Shaping::Advanced);

    ShapedText {
        lines: buffer
            .layout_runs()
            .map(|run| ShapedLine {
                glyphs: run.glyphs.to_vec(),
                line_y: run.line_y,
            })
            .collect(),
    }
}

/// Rasterize the glyph identified by `cache_key`. Returns `None` for
/// glyphs that have no visual representation (zero-width characters,
/// missing glyph in the chosen font, etc.) — this is an expected outcome,
/// not an error.
pub fn rasterize_glyph(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    cache_key: CacheKey,
) -> Option<RasterizedGlyph> {
    let image = swash_cache.get_image_uncached(font_system, cache_key)?;
    if image.placement.width == 0 || image.placement.height == 0 {
        return None;
    }
    let format = match image.content {
        SwashContent::Color => GlyphFormat::Rgba,
        // SubpixelMask is RGB coverage for LCD subpixel rendering. We don't
        // do LCD subpixel AA (we use 3-step horizontal positioning instead,
        // landing in the next commit), so collapse it onto the alpha path.
        SwashContent::Mask | SwashContent::SubpixelMask => GlyphFormat::Alpha,
    };
    Some(RasterizedGlyph {
        data: image.data,
        width: image.placement.width,
        height: image.placement.height,
        left: image.placement.left,
        top: image.placement.top,
        format,
    })
}
