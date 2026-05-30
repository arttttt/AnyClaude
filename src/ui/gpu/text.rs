//! The terminal + chrome text-rasterization resources, bundled. cosmic-text
//! rasterizes glyphs against `font_system`; `swash_cache` holds the bitmap data
//! destined for the atlas; the two shape caches are family-scoped (Monospace for
//! terminal cells, SansSerif for chrome / popups). `palette` is the ANSI colour
//! table; `cell_metrics` caches the measured monospace cell size (invalidated on
//! a scale change). Grouped so `GpuApp` isn't littered with six text handles —
//! the coordinator's `cell_metrics()` + render code reach the fields directly.

use term_core::AnsiPalette;
use term_gpu::{CellMetrics, FontFamily, FontSystem, SwashCache, TextShapeCache};

pub(super) struct TextResources {
    pub(super) font_system: FontSystem,
    pub(super) swash_cache: SwashCache,
    /// Monospace shape cache for terminal cells.
    pub(super) shape_cache: TextShapeCache,
    /// SansSerif shape cache for chrome / popups (family-scoped, so separate).
    pub(super) ui_shape_cache: TextShapeCache,
    pub(super) palette: AnsiPalette,
    /// Cached measured monospace cell size; `None` until first computed / after
    /// a scale-factor change.
    pub(super) cell_metrics: Option<CellMetrics>,
}

impl TextResources {
    pub(super) fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            shape_cache: TextShapeCache::with_family(FontFamily::Monospace),
            ui_shape_cache: TextShapeCache::with_family(FontFamily::SansSerif),
            palette: AnsiPalette::default_dark(),
            cell_metrics: None,
        }
    }
}
