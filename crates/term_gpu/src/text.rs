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
    fontdb, Attrs, Buffer, CacheKey, Family, FontSystem, LayoutGlyph, Metrics, Shaping, Style,
    SwashCache, SwashContent, Weight,
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

/// Single-codepoint glyph resolved via direct cmap lookup, bypassing
/// cosmic-text's shaper. Mirrors Warp's `glyph_for_char` hot path: caller
/// gets enough state to construct a `CacheKey` and place the glyph at a
/// known origin without paying for `String` allocation, BiDi analysis, or
/// `ShapeBuffer` reuse.
///
/// `baseline_y_physical` is the offset from the cell's top edge to the
/// glyph's baseline, in physical pixels at the requested
/// `font_size * scale_factor`. Add it to the cell's top-left physical Y
/// to get the position to pass to `CacheKey::new`.
#[derive(Debug, Clone, Copy)]
pub struct CharGlyph {
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub baseline_y_physical: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FaceKey {
    weight: Weight,
    style: Style,
}

#[derive(Clone, Copy)]
struct FaceInfo {
    font_id: fontdb::ID,
    /// Ascender in em-units (font units / units_per_em). Multiply by
    /// `font_size_physical` to get pixels above the baseline.
    ascent_em: f32,
    /// Absolute descender depth in em-units (always positive).
    descent_em: f32,
    /// Recommended line gap in em-units (font's `hhea.lineGap`).
    line_gap_em: f32,
    /// Advance width of the canonical "M" glyph in em-units, used to
    /// derive the monospace cell width. Falls back to 0.6 em when the
    /// glyph is missing from the face.
    em_width: f32,
}

/// Per-face metrics, scaled to the requested physical font size.
/// Returned by [`TextShapeCache::face_metrics`] so renderers can compute
/// cell dimensions and baseline placement from the font's own
/// ascender / descender / line-gap, not an arbitrary multiplier.
#[derive(Debug, Clone, Copy)]
pub struct FaceMetrics {
    pub ascent_physical: f32,
    pub descent_physical: f32,
    pub line_gap_physical: f32,
    pub em_width_physical: f32,
}

impl FaceMetrics {
    /// Standard terminal cell height = ascent + descent + line_gap,
    /// ceil'd to the next integer pixel so the baseline is on a pixel
    /// boundary. Matches Warp's grid_size_util formula.
    pub fn cell_height(&self) -> f32 {
        (self.ascent_physical + self.descent_physical + self.line_gap_physical)
            .ceil()
            .max(1.0)
    }

    pub fn cell_width(&self) -> f32 {
        self.em_width_physical.round().max(1.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct CharGlyphKey {
    ch: char,
    font_id: fontdb::ID,
}

struct CachedCharGlyph {
    glyph_id: Option<u16>,
    last_used_frame: u32,
}

use crate::atlas::{GlyphFormat, RasterizedGlyph};

/// Drop entries from the shape cache after this many consecutive frames
/// without a hit. Cheap to keep entries around (text is small), but unbounded
/// growth becomes a problem when callers shape unique text per row.
const SHAPE_CACHE_MAX_UNUSED_FRAMES: u32 = 60;

/// Monochrome symbol fallback face, tried for a char the primary face lacks
/// BEFORE cosmic-text's automatic fallback. cosmic-text's fallback on macOS
/// routes symbols like U+23FA (⏺, the Claude Code bullet) to the colour emoji
/// font, which rasterizes a boxed colour bitmap; "Apple Symbols" is a monochrome
/// symbol font that covers those ranges, so its glyph comes back as a `Mask` and
/// the renderer tints it to the text colour (a clean filled circle). Mirrors
/// Warp, which force-appends Apple Symbols ahead of colour emoji in its cascade.
/// Absent off macOS → the query returns `None` and the path no-ops.
const SYMBOL_FALLBACK_FAMILY: &str = "Apple Symbols";

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
    weight: Weight,
    style: Style,
}

struct CachedShape {
    text: ShapedText,
    last_used_frame: u32,
}

/// Caches shaped text in two tiers:
///
/// 1. **Char fast-path** (`shape_char`): single-codepoint cells (the 99%
///    case for terminal grids) resolve directly through `ttf_parser`'s
///    `cmap`, bypassing cosmic-text's `Buffer`. Key is `(char, font_id)`,
///    no `String` allocation, no BiDi/shaping cost. Mirrors Warp's
///    `CellGlyphCache.glyph_cache: HashMap<(char, FontId), …>`.
///
/// 2. **String slow-path** (`shape`): combining clusters, ligatures, mixed
///    scripts. Goes through full cosmic-text shaping with a `String` cache
///    key — the unavoidable cost when shaping is actually needed. Mirrors
///    Warp's `string_cache: HashMap<(String, FontId), …>`.
///
/// Both tiers are evicted by frame counter — call `end_frame()` once per
/// rendered frame so entries unused for `SHAPE_CACHE_MAX_UNUSED_FRAMES`
/// drop out. Primary face lookups (`FaceKey → FaceInfo`) are kept for the
/// lifetime of the cache (handful of entries, no growth concern).
pub struct TextShapeCache {
    entries: HashMap<ShapeKey, CachedShape>,
    char_entries: HashMap<CharGlyphKey, CachedCharGlyph>,
    face_cache: HashMap<FaceKey, Option<FaceInfo>>,
    /// Resolved [`SYMBOL_FALLBACK_FAMILY`] font id, looked up once. Outer
    /// `Option` = "resolved yet"; inner = "found in the font db".
    symbol_face: Option<Option<fontdb::ID>>,
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
            char_entries: HashMap::new(),
            face_cache: HashMap::new(),
            symbol_face: None,
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
    ///
    /// `weight` and `style` are forwarded to cosmic-text's `Attrs` and form
    /// part of the cache key — bold vs regular, italic vs upright cache
    /// separately and may resolve to different font faces.
    pub fn shape(
        &mut self,
        font_system: &mut FontSystem,
        text: &str,
        font_size: f32,
        scale_factor: f32,
        wrap_width: Option<f32>,
        weight: Weight,
        style: Style,
    ) -> &ShapedText {
        let key = ShapeKey {
            text: text.to_string(),
            font_size_bits: font_size.to_bits(),
            scale_factor_bits: scale_factor.to_bits(),
            wrap_width_bits: wrap_width.map(|w| w.to_bits()),
            weight,
            style,
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
                weight,
                style,
            ),
            last_used_frame: frame,
        });
        entry.last_used_frame = frame;
        &entry.text
    }

    /// Fast-path for single-codepoint cells: resolve `ch` to a glyph via
    /// the primary font face's `cmap` table directly, with no shaping.
    /// Returns `None` when the resolved face has no glyph for `ch` —
    /// callers should fall back to [`Self::shape`] (which engages
    /// cosmic-text's font fallback) or emit a missing-glyph indicator.
    ///
    /// `weight` and `style` select among faces in the primary family
    /// (e.g. bold/italic variants). The chosen face is cached per
    /// `(weight, style)` pair for the lifetime of the cache.
    ///
    /// Returned `baseline_y_physical` is the ascent of the primary face
    /// at the requested physical font size — add it to the cell's
    /// top-edge physical Y to get the baseline coordinate to pass into
    /// [`cosmic_text::CacheKey::new`].
    ///
    /// Whitespace characters (space, tab) ARE resolved if present in the
    /// font — most fonts include a zero-extent glyph for `U+0020` that
    /// rasterizes to a zero-pixel image. Callers that already know the
    /// cell is blank should skip this call entirely (the existing
    /// `is_blank` gate at the renderer covers this).
    pub fn shape_char(
        &mut self,
        font_system: &mut FontSystem,
        ch: char,
        font_size: f32,
        scale_factor: f32,
        weight: Weight,
        style: Style,
    ) -> Option<CharGlyph> {
        let face_info = self.resolve_face(font_system, weight, style)?;
        // Baseline is the primary face's ascent so the glyph sits on the cell
        // baseline regardless of which face actually supplies it.
        let baseline_y_physical = face_info.ascent_em * font_size * scale_factor;

        // Primary face first.
        if let Some(glyph_id) = self.cmap_glyph(font_system, ch, face_info.font_id) {
            return Some(CharGlyph {
                font_id: face_info.font_id,
                glyph_id,
                baseline_y_physical,
            });
        }

        // The primary face lacks this glyph. Try the monochrome symbol fallback
        // (see [`SYMBOL_FALLBACK_FAMILY`]) before returning `None` and letting
        // the caller drop to cosmic-text's emoji-prone fallback — this keeps
        // symbols like U+23FA a tinted `Mask`, not a boxed colour emoji. Real
        // emoji / CJK aren't in this font, so they still fall through correctly.
        if let Some(sym_id) = self.resolve_symbol_fallback(font_system) {
            if let Some(glyph_id) = self.cmap_glyph(font_system, ch, sym_id) {
                return Some(CharGlyph {
                    font_id: sym_id,
                    glyph_id,
                    baseline_y_physical,
                });
            }
        }
        None
    }

    /// Cached single-codepoint `cmap` lookup in a specific face. Returns the
    /// glyph id, or `None` when the face has no glyph for `ch`.
    fn cmap_glyph(
        &mut self,
        font_system: &mut FontSystem,
        ch: char,
        font_id: fontdb::ID,
    ) -> Option<u16> {
        let frame = self.frame;
        let entry = self
            .char_entries
            .entry(CharGlyphKey { ch, font_id })
            .or_insert_with(|| {
                let glyph_id = font_system
                    .get_font(font_id)
                    .and_then(|font| font.rustybuzz().glyph_index(ch).map(|g| g.0));
                CachedCharGlyph {
                    glyph_id,
                    last_used_frame: frame,
                }
            });
        entry.last_used_frame = frame;
        entry.glyph_id
    }

    /// Resolve [`SYMBOL_FALLBACK_FAMILY`] in the font db, once. `None` when it's
    /// not installed (e.g. off macOS) — the symbol-fallback path then no-ops.
    fn resolve_symbol_fallback(&mut self, font_system: &mut FontSystem) -> Option<fontdb::ID> {
        if let Some(cached) = self.symbol_face {
            return cached;
        }
        let query = fontdb::Query {
            families: &[Family::Name(SYMBOL_FALLBACK_FAMILY)],
            weight: Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: Style::Normal,
        };
        let id = font_system.db().query(&query);
        self.symbol_face = Some(id);
        id
    }

    /// Scaled metrics for the primary face under the given
    /// `(family, weight, style)` at `font_size * scale_factor`.
    /// Returns `None` when face resolution fails (no matching face
    /// in the system font database, or zero units-per-em).
    pub fn face_metrics(
        &mut self,
        font_system: &mut FontSystem,
        font_size: f32,
        scale_factor: f32,
        weight: Weight,
        style: Style,
    ) -> Option<FaceMetrics> {
        let face = self.resolve_face(font_system, weight, style)?;
        let font_size_physical = font_size * scale_factor;
        Some(FaceMetrics {
            ascent_physical: face.ascent_em * font_size_physical,
            descent_physical: face.descent_em * font_size_physical,
            line_gap_physical: face.line_gap_em * font_size_physical,
            em_width_physical: face.em_width * font_size_physical,
        })
    }

    fn resolve_face(
        &mut self,
        font_system: &mut FontSystem,
        weight: Weight,
        style: Style,
    ) -> Option<FaceInfo> {
        let key = FaceKey { weight, style };
        if let Some(cached) = self.face_cache.get(&key) {
            return *cached;
        }
        let info = resolve_primary_face(font_system, &self.family, weight, style);
        self.face_cache.insert(key, info);
        info
    }

    /// Advance the frame counter and evict entries that have not been
    /// requested in the last `SHAPE_CACHE_MAX_UNUSED_FRAMES` frames.
    /// Evicts both string and char caches. Face cache is kept for the
    /// lifetime of the cache — there are at most a handful of entries
    /// (one per `(weight, style)`), so unbounded growth is not a concern.
    pub fn end_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        let now = self.frame;
        self.entries
            .retain(|_, c| now.wrapping_sub(c.last_used_frame) <= SHAPE_CACHE_MAX_UNUSED_FRAMES);
        self.char_entries
            .retain(|_, c| now.wrapping_sub(c.last_used_frame) <= SHAPE_CACHE_MAX_UNUSED_FRAMES);
    }
}

/// Query the primary font face for the given `(family, weight, style)`
/// via `fontdb`. Reads `ascender / units_per_em` from the face's
/// `ttf_parser` view so callers can compute baseline offsets without a
/// shape pass.
fn resolve_primary_face(
    font_system: &mut FontSystem,
    family: &FontFamily,
    weight: Weight,
    style: Style,
) -> Option<FaceInfo> {
    let cosmic_family = family.as_cosmic();
    let query = fontdb::Query {
        families: &[cosmic_family],
        weight,
        stretch: fontdb::Stretch::Normal,
        style,
    };
    let id = font_system.db().query(&query)?;
    let font = font_system.get_font(id)?;
    let face = font.rustybuzz();
    let upem = face.units_per_em() as f32;
    if upem <= 0.0 {
        return None;
    }
    let ascent_em = face.ascender() as f32 / upem;
    // `descender()` is signed and conventionally negative — store its
    // absolute value so callers can add it cleanly to ascent + line_gap.
    let descent_em = -(face.descender() as f32) / upem;
    let line_gap_em = face.line_gap() as f32 / upem;
    let em_width = face
        .glyph_index('M')
        .and_then(|g| face.glyph_hor_advance(g))
        .map(|adv| adv as f32 / upem)
        .unwrap_or(0.6);
    Some(FaceInfo {
        font_id: id,
        ascent_em,
        descent_em,
        line_gap_em,
        em_width,
    })
}

fn shape_text_inline(
    font_system: &mut FontSystem,
    text: &str,
    font_size_physical: f32,
    wrap_width_physical: Option<f32>,
    family: &FontFamily,
    weight: Weight,
    style: Style,
) -> ShapedText {
    let line_height = font_size_physical * 1.3;
    let metrics = Metrics::new(font_size_physical, line_height);
    let mut buffer = Buffer::new_empty(metrics);
    buffer.set_size(font_system, wrap_width_physical, None);
    let attrs = Attrs::new()
        .family(family.as_cosmic())
        .weight(weight)
        .style(style);
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
