//! R4 property test — the Phase A acceptance gate (design §1 R4, §15).
//!
//! R4: destroying the entire retained tree and rebuilding it from a view yields
//! output identical to incrementally reconciling the previous tree into the new
//! view, when compared on **CPU-computable layout + color + glyph identity**
//! (geometry, color, cosmic-text `CacheKey`), normalizing away atlas UVs and
//! frame counters. The headless `paint_cpu` produces exactly that normalized
//! output, so we compare `CpuPaint == CpuPaint`.
//!
//! Equality is EXACT f32 bit-equality, by design: both paths run the identical
//! `measure`/`place` arithmetic in identical order, so any divergence is a real
//! reconcile bug, not float noise. `GlyphRecord` carries only the
//! integer-quantized cosmic-text `CacheKey` (subpixel-binned) + color, never
//! atlas UVs or frame counters, so the only "float" compared is already
//! quantized. Do NOT loosen this to an epsilon compare — fix layout determinism
//! instead if a future change makes summation order data-dependent.
//!
//! Beyond the paint compare, the test also asserts (to defeat a soft
//! tautology where both paths re-run layout and mask a reconcile that wrote
//! nothing): (1) structural arena equality (node.kind + child counts) after
//! reconcile vs rebuild; (2) `live_count` equality + steady-state over
//! grow/shrink sequences (the only assertion that fails on a teardown leak);
//! (3) R8 reorder identity at the `WidgetId` level (a stale `NodeId` captured
//! before a reorder no longer resolves to its old content).

use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache};
use term_ui::{
    build_root, collect_focus_order, measure, paint_cpu, place, reconcile_root, Block, BlockStyle,
    BoxView, CpuPaint, Element, Insets, NodeId, NodeKind, RetainedTree, SizeConstraint, Sizing,
    Stack, Text, WidgetId,
};

const FS: f32 = 18.0;
const SF: f32 = 2.0;
const WHITE: [f32; 4] = [0.9, 0.9, 0.95, 1.0];
const RED: [f32; 4] = [0.9, 0.2, 0.2, 1.0];
const BLOCK_BG: [f32; 4] = [0.1, 0.1, 0.12, 1.0];

fn fonts() -> (FontSystem, TextShapeCache) {
    (FontSystem::new(), TextShapeCache::with_family(FontFamily::SansSerif))
}

/// Lay out a view into a fresh tree from scratch and paint it on the CPU.
fn rebuild<E: Element>(
    view: &E,
    fonts: &mut FontSystem,
    shape: &mut TextShapeCache,
) -> (RetainedTree, CpuPaint) {
    let mut tree = RetainedTree::new();
    let root = build_root(&mut tree, view);
    layout_and_paint(&mut tree, root, fonts, shape)
}

/// Build `prev` into a tree, reconcile it to `next`, then paint on the CPU.
fn incremental<E: Element>(
    prev: &E,
    next: &E,
    fonts: &mut FontSystem,
    shape: &mut TextShapeCache,
) -> (RetainedTree, CpuPaint) {
    let mut tree = RetainedTree::new();
    let root = build_root(&mut tree, prev);
    let root = reconcile_root(&mut tree, root, prev, next);
    layout_and_paint(&mut tree, root, fonts, shape)
}

fn layout_and_paint(
    tree: &mut RetainedTree,
    root: NodeId,
    fonts: &mut FontSystem,
    shape: &mut TextShapeCache,
) -> (RetainedTree, CpuPaint) {
    let constraint = SizeConstraint::loose(Vec2::new(800.0, 600.0));
    measure(tree, root, constraint, fonts, shape, SF);
    place(tree, root, Vec2::new(10.0, 10.0));
    let mut out = CpuPaint::default();
    paint_cpu(tree, root, &mut out, fonts, shape, SF);
    // Move the tree out so callers can inspect arena state (live_count etc.).
    (std::mem::replace(tree, RetainedTree::new()), out)
}

/// Assert the two trees are structurally identical (kind + child count at each
/// position), proving reconcile actually propagated the new view's shape rather
/// than relying on the test's own layout pass to paper over it.
fn assert_same_structure(a: &RetainedTree, b: &RetainedTree) {
    let ra = a.root.expect("a root");
    let rb = b.root.expect("b root");
    assert_same_structure_at(a, ra, b, rb);
}

fn assert_same_structure_at(a: &RetainedTree, ai: NodeId, b: &RetainedTree, bi: NodeId) {
    let na = a.node(ai);
    let nb = b.node(bi);
    assert_eq!(na.kind, nb.kind, "node kind diverged (reconcile wrote wrong kind)");
    assert_eq!(
        na.children.len(),
        nb.children.len(),
        "child count diverged at a node"
    );
    assert_eq!(
        na.per_child_sizing, nb.per_child_sizing,
        "per_child_sizing diverged"
    );
    for (&ca, &cb) in na.children.iter().zip(nb.children.iter()) {
        assert_same_structure_at(a, ca, b, cb);
    }
}

/// The core R4 assertion for one (prev -> next) mutation: incremental == rebuild
/// on CPU paint AND on arena structure AND on live-slot count (leak check).
fn assert_r4<E: Element>(prev: &E, next: &E) {
    let (mut f, mut s) = fonts();
    let (rebuilt_tree, rebuilt) = rebuild(next, &mut f, &mut s);
    let (inc_tree, inc) = incremental(prev, next, &mut f, &mut s);

    assert_eq!(
        inc, rebuilt,
        "R4: incremental reconcile CpuPaint != rebuild-from-scratch CpuPaint"
    );
    assert_same_structure(&inc_tree, &rebuilt_tree);
    assert_eq!(
        inc_tree.live_count(),
        rebuilt_tree.live_count(),
        "R4 leak: incremental tree has a different live-slot count than rebuild \
         (a freed node leaked or a slot failed to free)"
    );
}

fn block(child: impl Element) -> Block {
    Block::new(
        BlockStyle {
            background: BLOCK_BG,
            border_color: WHITE,
            border_width: 1.0,
            padding: Insets::all(4.0),
            shadow: None,
        },
        child,
    )
}

// ─────────────────────────── property cases ────────────────────────────

#[test]
fn r4_text_edit() {
    let prev = Stack::vstack().child(Text::new("hello", FS, WHITE));
    let next = Stack::vstack().child(Text::new("hello world!", FS, WHITE));
    assert_r4(&prev, &next);
}

#[test]
fn r4_color_change() {
    // Geometry is identical; only color changes. This case fails unless
    // reconcile actually wrote the new TextStyle into the slot's kind.
    let prev = Stack::vstack().child(Text::new("colored", FS, WHITE));
    let next = Stack::vstack().child(Text::new("colored", FS, RED));
    assert_r4(&prev, &next);

    // Tighter check: the incremental tree's painted glyph colors must equal the
    // MUTATED color (RED), proving reconcile propagated the change rather than
    // leaving the original WHITE in place.
    let (mut f, mut s) = fonts();
    let (_inc_tree, inc) = incremental(&prev, &next, &mut f, &mut s);
    assert!(!inc.glyphs.is_empty(), "expected painted glyphs");
    assert!(
        inc.glyphs.iter().all(|g| g.color == RED),
        "reconcile did not propagate the color change into the painted glyphs"
    );
}

#[test]
fn r4_weight_and_italic_change() {
    let prev = Stack::vstack().child(Text::new("styled", FS, WHITE));
    let next = Stack::vstack().child(Text::new("styled", FS, WHITE).weight(700).italic(true));
    assert_r4(&prev, &next);
}

#[test]
fn r4_child_insert() {
    let prev = Stack::vstack().gap(6.0).child(Text::new("a", FS, WHITE));
    let next = Stack::vstack()
        .gap(6.0)
        .child(Text::new("a", FS, WHITE))
        .child(Text::new("b", FS, WHITE));
    assert_r4(&prev, &next);
}

#[test]
fn r4_child_delete() {
    let prev = Stack::vstack()
        .gap(6.0)
        .child(Text::new("a", FS, WHITE))
        .child(Text::new("b", FS, WHITE))
        .child(Text::new("c", FS, WHITE));
    let next = Stack::vstack().gap(6.0).child(Text::new("a", FS, WHITE));
    assert_r4(&prev, &next);
}

#[test]
fn r4_reorder() {
    // R8: the same two logical labels swap order. Identity is keyed by the
    // stable id-path WidgetId, which survives the reorder.
    let prev = Stack::hstack()
        .gap(8.0)
        .child(Text::new("AAA", FS, WHITE).id(WidgetId::from_path(&[7, 0])))
        .child(Text::new("B", FS, RED).id(WidgetId::from_path(&[7, 1])));
    let next = Stack::hstack()
        .gap(8.0)
        .child(Text::new("B", FS, RED).id(WidgetId::from_path(&[7, 1])))
        .child(Text::new("AAA", FS, WHITE).id(WidgetId::from_path(&[7, 0])));
    assert_r4(&prev, &next);
}

#[test]
fn r4_type_change_forces_rebuild() {
    // A child changes concrete type (Text -> Stack). The splice must
    // delete+insert (teardown the Text slot, build a fresh Stack subtree).
    let prev = Stack::vstack().child(Text::new("leaf", FS, WHITE));
    let next = Stack::vstack().child(Stack::hstack().child(Text::new("nested", FS, RED)));
    assert_r4(&prev, &next);
}

#[test]
fn r4_stack_to_block_swap() {
    // Root-level single child swaps Stack <-> Block (type change in a Block's
    // single-child splice slot).
    let prev = block(Stack::hstack().child(Text::new("x", FS, WHITE)));
    let next = block(Block::new(
        BlockStyle {
            background: RED,
            border_color: WHITE,
            border_width: 0.0,
            padding: Insets::all(2.0),
            shadow: None,
        },
        Text::new("x", FS, WHITE),
    ));
    assert_r4(&prev, &next);
}

#[test]
fn r4_flex_layout_preserved() {
    // Flex distribution must reconcile identically (geometry-heavy case).
    let prev = Stack::hstack()
        .child_sized(Text::new("L", FS, WHITE), Sizing::Fixed(40.0))
        .child_boxed(Box::new(Text::new("M", FS, RED)) as BoxView, Sizing::Flex(1.0))
        .spacer(Sizing::Flex(2.0));
    let next = Stack::hstack()
        .child_sized(Text::new("L2", FS, WHITE), Sizing::Fixed(60.0))
        .child_boxed(Box::new(Text::new("MM", FS, RED)) as BoxView, Sizing::Flex(1.0))
        .spacer(Sizing::Flex(2.0));
    assert_r4(&prev, &next);
}

// ─────────────────────── R8 reorder identity (slot-level) ───────────────

#[test]
fn r8_reorder_identity_at_widget_id() {
    let wid_a = WidgetId::from_path(&[9, 0]);
    let wid_b = WidgetId::from_path(&[9, 1]);

    let mut tree = RetainedTree::new();
    let prev = Stack::hstack()
        .child(Text::new("AAA", FS, WHITE).id(wid_a))
        .child(Text::new("B", FS, RED).id(wid_b));
    let root = build_root(&mut tree, &prev);

    // Capture A's arena slot BEFORE the reorder.
    let a_slot_before = tree.resolve_widget(wid_a).expect("A mapped before reorder");
    let a_text_before = match &tree.node(a_slot_before).kind {
        NodeKind::Text(t) => t.text.clone(),
        _ => panic!("A should be a Text node"),
    };
    assert_eq!(a_text_before, "AAA");

    // Reorder [A, B] -> [B, A]. The positional splice reuses slot 0 for B's
    // content and slot 1 for A's content; identity follows the stable WidgetId.
    let next = Stack::hstack()
        .child(Text::new("B", FS, RED).id(wid_b))
        .child(Text::new("AAA", FS, WHITE).id(wid_a));
    reconcile_root(&mut tree, root, &prev, &next);

    // WidgetId identity survives the reorder: each id still resolves, and to a
    // slot now holding that logical widget's content.
    let a_slot_after = tree.resolve_widget(wid_a).expect("A still mapped after reorder");
    let b_slot_after = tree.resolve_widget(wid_b).expect("B still mapped after reorder");
    let a_text_after = match &tree.node(a_slot_after).kind {
        NodeKind::Text(t) => t.text.clone(),
        _ => panic!("A should still be a Text node"),
    };
    let b_text_after = match &tree.node(b_slot_after).kind {
        NodeKind::Text(t) => t.text.clone(),
        _ => panic!("B should still be a Text node"),
    };
    assert_eq!(a_text_after, "AAA", "A's content followed its WidgetId across reorder");
    assert_eq!(b_text_after, "B", "B's content followed its WidgetId across reorder");

    // The positional splice put A's content in the slot that previously held A's
    // OLD content only if positions matched; after the swap A now lives in the
    // slot that was B's. Either way, the OLD captured NodeId for A must now
    // resolve to the slot that physically holds A's *position*, which after the
    // swap is B's content — i.e. the stale positional handle does NOT track A.
    let stale_text = match &tree.node(a_slot_before).kind {
        NodeKind::Text(t) => t.text.clone(),
        _ => panic!("slot still a Text node"),
    };
    assert_eq!(
        stale_text, "B",
        "the NodeId captured for A before the reorder now resolves to B's content \
         (positional slot reuse) — proving NodeId is NOT a stable identity"
    );
    assert_ne!(
        a_slot_before, a_slot_after,
        "A's stable WidgetId now maps to a different arena slot than before the reorder"
    );
}

// ───────────────────────── leak / slot-reuse over time ──────────────────

#[test]
fn r4_no_leak_over_grow_shrink_sequence() {
    // `Stack` owns `BoxView`s and is not `Clone`, so each reconcile pass
    // re-authors fresh `prev`/`next` views from these factories (the same
    // pattern Phase B's `view(&AppState)` will follow: a view is rebuilt, never
    // retained between frames).
    let small = || Stack::vstack().child(Text::new("a", FS, WHITE));
    let large = || {
        Stack::vstack()
            .child(Text::new("a", FS, WHITE))
            .child(Text::new("b", FS, WHITE))
            .child(
                Stack::hstack()
                    .child(Text::new("c", FS, RED))
                    .child(Text::new("d", FS, RED)),
            )
    };

    let mut tree = RetainedTree::new();
    let root = build_root(&mut tree, &small());
    let baseline_live = tree.live_count();

    // Grow then shrink several times; after each shrink-back the live count must
    // return to baseline and capacity must not grow unboundedly (free-list reuse).
    for _ in 0..5 {
        reconcile_root(&mut tree, root, &small(), &large());
        reconcile_root(&mut tree, root, &large(), &small());
        assert_eq!(
            tree.live_count(),
            baseline_live,
            "live_count did not return to baseline after shrink (leak)"
        );
    }

    // Capacity is bounded by the largest tree ever built (6 nodes: vstack + 2
    // texts + hstack + 2 texts), not by the number of grow/shrink cycles —
    // freed slots are recycled via the free list.
    assert!(
        tree.slot_capacity() <= 6,
        "slot capacity grew unboundedly across reconciles ({} slots) — free list not reused",
        tree.slot_capacity()
    );
}

// ─────────────────── focus_order scaffolding (§7, bucket 2) ──────────────

#[test]
fn focus_order_is_depth_first_over_focusable_widgets() {
    // collect_focus_order's only consumer (Phase A): prove it walks depth-first
    // and emits the stable WidgetIds of focusable widgets in tree order, and
    // that the order survives a reorder (R8 — identity follows the WidgetId).
    let wid0 = WidgetId::from_path(&[1, 0]);
    let wid1 = WidgetId::from_path(&[1, 1]);

    let mut tree = RetainedTree::new();
    let prev = Stack::vstack()
        .child(Text::new("focusable-0", FS, WHITE).id(wid0).focusable(true))
        .child(Text::new("not-focusable", FS, WHITE)) // skipped (focusable=false)
        .child(
            Stack::hstack()
                .child(Text::new("focusable-1", FS, RED).id(wid1).focusable(true)),
        );
    let root = build_root(&mut tree, &prev);

    let mut order = Vec::new();
    collect_focus_order(&tree, root, &mut order);
    assert_eq!(order, vec![wid0, wid1], "focus_order should be depth-first focusables");

    // Reorder the two focusable widgets; the collected order tracks tree
    // position but each entry is the same stable WidgetId.
    let next = Stack::vstack()
        .child(
            Stack::hstack()
                .child(Text::new("focusable-1", FS, RED).id(wid1).focusable(true)),
        )
        .child(Text::new("not-focusable", FS, WHITE))
        .child(Text::new("focusable-0", FS, WHITE).id(wid0).focusable(true));
    reconcile_root(&mut tree, root, &prev, &next);

    let mut order2 = Vec::new();
    collect_focus_order(&tree, root, &mut order2);
    assert_eq!(order2, vec![wid1, wid0], "focus_order reflects the new tree order");
}

// ───────────────────── layout precondition (debug guard) ────────────────

#[test]
#[should_panic(expected = "infinite main-axis")]
fn flex_on_infinite_main_axis_panics() {
    // §5 hard invariant: measuring a flex-bearing stack under an infinite
    // main-axis constraint is a caller bug. The debug_assert must fire (tests
    // run in debug), proving the invariant is enforced, not silently clamped.
    let (mut f, mut s) = fonts();
    let view = Stack::hstack().child_sized(Text::new("flex", FS, WHITE), Sizing::Flex(1.0));
    let mut tree = RetainedTree::new();
    let root = build_root(&mut tree, &view);
    // Horizontal main axis with an infinite max width.
    let constraint = SizeConstraint::loose(Vec2::new(f32::INFINITY, 600.0));
    measure(&mut tree, root, constraint, &mut f, &mut s, SF);
}
