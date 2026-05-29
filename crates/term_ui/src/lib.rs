//! `term_ui` — the retained + reactive UI engine that sits between AppState
//! (Phase B+) and `term_gpu`. Phase A is the engine core: a flat generational
//! arena, a `View`/`Element` trait (build/reconcile/teardown — NO event, R7),
//! an `ElementSplice` list diff, a Flex-lite layout engine (index-based
//! measure → place → paint free fns), and the two new caret helpers built on
//! `term_gpu::ShapedLine`.
//!
//! Design: `docs/design/term-ui-design.md` (§1 invariants R1–R15, §4 view
//! model, §5 layout, §14 ownership, §15 Phase A).
//!
//! Scope (KISS/YAGNI, R13): Phase A deliberately contains NO `AppState`, NO
//! event routing, NO coordinator, NO popups, NO `Msg`/`apply`. Those are
//! Phase B+. This crate proves the engine on a toy and gates on the R4
//! property test (rebuild-from-scratch == incremental on CPU-computable
//! layout + color + glyph identity).
//!
//! term_ui layers on term_gpu and consumes its instance/text surface; it
//! re-implements none of it (R9).

pub mod anim;
pub mod arena;
pub mod geometry;
pub mod id;
pub mod layout;
pub mod paint;
pub mod splice;
pub mod text_helpers;
pub mod view;

// ── public surface (kept reachable so new types don't trip dead_code) ──

pub use anim::{apply_overlay_alpha, ease_in_out, ease_out, lerp, linear};
pub use arena::{
    BlockShadow, BlockStyle, Node, NodeKind, RetainedTree, StackStyle, TextStyle,
};
pub use geometry::{
    Axis, Bounds, CrossAxis, Insets, MainAxis, SizeConstraint, Sizing,
};
pub use id::{NodeId, WidgetId};
pub use layout::{line_height, measure, measure_text, place, place_centered};
pub use paint::{
    block_shadow, paint, paint_cpu, CpuPaint, GlyphRecord, PaintOutput, RectRecord,
};
pub use splice::reconcile_children;
pub use text_helpers::{byte_at_x, caret_x};
pub use view::{
    build_root, collect_focus_order, free_subtree, reconcile_root, Block, BoxView, Element,
    Spacer, Stack, Text,
};
