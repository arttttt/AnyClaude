//! GPU-accelerated terminal renderer.
//!
//! Phase 3.5 prototype: validates pixel-based scroll with Warp-style momentum
//! before the full term_gpu crate is fleshed out. See
//! `docs/design/gpu-terminal-scroll.md` for the design and
//! `examples/scroll_demo.rs` for the demo entry point.

pub mod atlas;
pub mod input;
pub mod instances;
pub mod panel_render;
pub mod paste;
pub mod pipeline;
pub mod renderer;
pub mod scroll;
pub mod selection;
pub mod text;

pub use atlas::{GlyphAtlas, GlyphFormat, PlacedGlyph, RasterizedGlyph, ShelfPacker};
pub use input::encode_key;
pub use instances::{GlyphInstance, RectInstance, Uniforms};
pub use panel_render::{
    build_cursor_rect, measure_cell_metrics, populate_panel, CellMetrics, PanelRect,
    CURSOR_COLOR, CURSOR_STROKE_PHYSICAL, DEFAULT_FG,
};
pub use paste::{encode_paste, shell_quote_path};
pub use renderer::GpuRenderer;
pub use scroll::{
    decay_velocity, ScrollState, ScrollVelocity, GESTURE_END_TIMEOUT, MOMENTUM_FRAME_INTERVAL,
    MOMENTUM_MIN_VELOCITY, MOMENTUM_THRESHOLD, NUM_PIXELS_PER_LINE,
};
pub use selection::{
    expand_line, expand_word, is_word_boundary, push_selection_rects, selection_to_text,
    CellPoint, Selection, SELECTION_COLOR, WORD_BOUNDARY_CHARS,
};
pub use text::{rasterize_glyph, CharGlyph, FontFamily, ShapedLine, ShapedText, TextShapeCache};

/// Re-exported cosmic-text types. Consumers wire these through
/// `TextShapeCache::shape` / `populate_panel` rather than importing
/// cosmic-text directly — keeps the dependency contained.
pub use cosmic_text::{FontSystem, Style, SwashCache, Weight};
