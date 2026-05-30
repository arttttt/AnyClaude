//! GPU-accelerated terminal renderer.
//!
//! Phase 3.5 prototype: validates pixel-based scroll with Warp-style momentum
//! before the full term_gpu crate is fleshed out. See
//! `docs/design/gpu-terminal-scroll.md` for the design and
//! `examples/scroll_demo.rs` for the demo entry point.

pub mod atlas;
pub mod input;
pub mod instances;
pub mod label;
pub mod panel_render;
pub mod paste;
pub mod pipeline;
pub mod renderer;
pub mod scroll;
pub mod selection;
pub mod text;

pub use atlas::{GlyphAtlas, GlyphFormat, PlacedGlyph, RasterizedGlyph, ShelfPacker};
pub use input::{
    encode_key, encode_motion_report, encode_mouse_report, encode_mouse_sgr, encode_mouse_x10,
    MouseButton, MouseEventKind,
};
pub use instances::{GlyphInstance, RectInstance, RenderLayer, ShadowInstance, Uniforms};
pub use label::{measure_label_width, push_label};
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
pub use text::{
    rasterize_glyph, CharGlyph, FaceMetrics, FontFamily, ShapedLine, ShapedText, TextShapeCache,
};

/// Re-exported cosmic-text types. Consumers wire these through
/// `TextShapeCache::shape` / `populate_panel` rather than importing
/// cosmic-text directly — keeps the dependency contained.
///
/// `CacheKey` / `LayoutGlyph` are part of the text surface term_ui builds on
/// (atlas-independent glyph identity for the R4 gate, and per-glyph cluster/x
/// offsets for caret mapping); re-exporting them here keeps term_ui from
/// depending on cosmic-text directly (R9).
pub use cosmic_text::{CacheKey, FontSystem, LayoutGlyph, Style, SwashCache, Weight};
