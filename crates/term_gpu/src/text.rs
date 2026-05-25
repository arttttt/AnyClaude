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

use cosmic_text::{CacheKey, FontSystem, SwashCache, SwashContent};

use crate::atlas::{GlyphFormat, RasterizedGlyph};

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
