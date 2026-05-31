//! The `View` trait + reconciliation (design §4).
//!
//! Authoring is declarative: `view(&AppState, frame_now)` (Phase B+) returns a
//! fresh view tree; in Phase A the toy/tests construct views by hand.
//! Execution is retained: each frame authors a NEW view tree and diffs it
//! against the persistent arena, applying ONLY deltas (Xilem `rebuild(prev,
//! self)`), reusing slots where a child's concrete type is unchanged and
//! inserting/deleting where it changed (the `ElementSplice` contract, §4).
//!
//! There is deliberately **no `event` method** (R7): event routing does not
//! exist in Phase A and, by design, never becomes a tree walk.
//!
//! Ownership model (the key to compiling cleanly under §14): a view tree is
//! **consumed** by build/reconcile. The retained "state" for the next frame is
//! simply the set of boxed views that were just installed — the new frame
//! diffs against them and replaces them. No `Rc`, no lazy state cells, no
//! `&mut Node` held across a recursive child call: every recursion takes
//! `(&mut RetainedTree, NodeId)` and re-borrows the arena per child.

use std::any::{Any, TypeId};

use crate::arena::{BlockStyle, Node, NodeKind, RetainedTree, StackStyle, TextStyle};
use crate::geometry::{Axis, CrossAxis, Insets, MainAxis, Sizing};
use crate::id::{NodeId, WidgetId};
use crate::modifier::Modifier;
use crate::splice::reconcile_children;

/// A retained, type-erased UI element. The view tree is a tree of these
/// (composites own `Vec<BoxView>` / one `BoxView`). Every method is
/// **consuming-by-tree-position**: `build` installs `self` into a fresh slot;
/// `reconcile` diffs `self` (the NEW view) against `prev` (last frame's view of
/// the same concrete type) at an existing slot; `teardown` frees the subtree.
///
/// Deviation from §4's documented Xilem four-method lifecycle (recorded so the
/// gap is explicit, not silent):
/// - **No `type State` / ViewState.** §4/§14 bill a per-node `ViewState` (memo
///   keys, cached measured-size handle). It is intentionally omitted in Phase A
///   per R13: the arena `Node` already carries the only per-node bookkeeping
///   that exists yet (`measured`/`bounds`), and there is no memo-key consumer.
///   It can be re-introduced in the phase that needs it.
/// - **`rebuild` is named `reconcile`** here (same role: diff prev→self,
///   apply deltas).
/// - **`as_any` / `concrete_type` are added** so the positional splice can do
///   its same-type check and the downcast in `reconcile` without a separate
///   type-tag field.
pub trait Element: Any {
    /// Allocate this element's slot (and subtree) in the arena. Returns the id.
    fn build(&self, tree: &mut RetainedTree) -> NodeId;

    /// Diff `self` against `prev` (guaranteed same concrete type by the splice)
    /// at slot `id`, applying only deltas. Re-borrows the arena per child.
    fn reconcile(&self, prev: &dyn Element, tree: &mut RetainedTree, id: NodeId);

    /// Free this element's subtree.
    fn teardown(&self, tree: &mut RetainedTree, id: NodeId);

    /// Upcast for the splice's same-type check and downcast in `reconcile`.
    fn as_any(&self) -> &dyn Any;

    /// The concrete type id, used by the splice to decide insert-vs-reuse.
    fn concrete_type(&self) -> TypeId {
        self.as_any().type_id()
    }
}

/// Boxed element — the node type the splice and composites store.
pub type BoxView = Box<dyn Element>;

/// Free a subtree bottom-up by walking the arena (no element type needed).
/// The fallback teardown path and scratch cleanup.
pub fn free_subtree(tree: &mut RetainedTree, id: NodeId) {
    if !tree.is_live(id) {
        return;
    }
    let children = tree.take_children(id);
    for child in &children {
        free_subtree(tree, *child);
    }
    tree.restore_children(id, children);
    tree.free(id);
}

/// Re-collect `focus_order` (depth-first) from a subtree's `focusable` flags.
/// Rebuilt each reconcile; stable `WidgetId`s only (§7).
pub fn collect_focus_order(tree: &RetainedTree, id: NodeId, out: &mut Vec<WidgetId>) {
    let node: &Node = tree.node(id);
    if node.focusable {
        if let Some(wid) = node.widget_id {
            out.push(wid);
        }
    }
    let children = node.children.clone();
    for child in children {
        collect_focus_order(tree, child, out);
    }
}

// ───────────────────────────── Text leaf ───────────────────────────────

/// A single line of variable-width text (§11 Text widget).
#[derive(Clone, PartialEq)]
pub struct Text {
    pub widget_id: Option<WidgetId>,
    pub style: TextStyle,
    /// Opt-in focus participation (§7). Written into the arena `Node` so
    /// `collect_focus_order` can derive `focus_order` (bucket 2). Event routing
    /// over that order is Phase B/D, not Phase A.
    pub focusable: bool,
}

impl Text {
    pub fn new(text: impl Into<String>, font_size: f32, color: [f32; 4]) -> Self {
        Self {
            widget_id: None,
            style: TextStyle {
                text: text.into(),
                font_size,
                weight: 400, // cosmic_text::Weight::NORMAL.0
                italic: false,
                color,
            },
            focusable: false,
        }
    }

    pub fn id(mut self, wid: WidgetId) -> Self {
        self.widget_id = Some(wid);
        self
    }

    pub fn weight(mut self, weight: u16) -> Self {
        self.style.weight = weight;
        self
    }

    pub fn italic(mut self, italic: bool) -> Self {
        self.style.italic = italic;
        self
    }

    /// Mark this text focusable so it appears in the tree's `focus_order` (§7).
    pub fn focusable(mut self, focusable: bool) -> Self {
        self.focusable = focusable;
        self
    }
}

impl Element for Text {
    fn build(&self, tree: &mut RetainedTree) -> NodeId {
        let id = tree.alloc(NodeKind::Text(self.style.clone()));
        tree.node_mut(id).focusable = self.focusable;
        if let Some(wid) = self.widget_id {
            tree.map_widget(wid, id);
        }
        id
    }

    fn reconcile(&self, prev: &dyn Element, tree: &mut RetainedTree, id: NodeId) {
        let prev = prev.as_any().downcast_ref::<Text>().expect("same type");
        if self.style != prev.style {
            tree.node_mut(id).kind = NodeKind::Text(self.style.clone());
            if let Some(wid) = self.widget_id {
                tree.mark_dirty(wid);
            }
        }
        tree.node_mut(id).focusable = self.focusable;
        if let Some(wid) = self.widget_id {
            tree.map_widget(wid, id);
        }
    }

    fn teardown(&self, tree: &mut RetainedTree, id: NodeId) {
        tree.free(id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ───────────────────────────── Spacer leaf ─────────────────────────────

/// Empty space; paints nothing. `Spacer::flex(w)` takes a share of leftover
/// main-axis space; `Spacer::fixed(px)` is a fixed gap (§11 Spacer/Fill).
///
/// `sizing` is the view-authored intent; the parent stack's `child_sizing[i]`
/// is what actually drives the spacer's main extent during layout (a Spacer
/// always measures to zero intrinsic size — see `measure_spacer`). The field is
/// retained so `Stack::spacer` can mirror it into `child_sizing` and so
/// `reconcile` can detect a changed spacer.
#[derive(Clone, PartialEq)]
pub struct Spacer {
    pub sizing: Sizing,
}

impl Spacer {
    pub fn flex(weight: f32) -> Self {
        Self { sizing: Sizing::Flex(weight) }
    }

    pub fn fill() -> Self {
        Self { sizing: Sizing::Fill }
    }

    pub fn fixed(px: f32) -> Self {
        Self { sizing: Sizing::Fixed(px) }
    }
}

impl Element for Spacer {
    fn build(&self, tree: &mut RetainedTree) -> NodeId {
        tree.alloc(NodeKind::Spacer(self.sizing))
    }

    fn reconcile(&self, prev: &dyn Element, tree: &mut RetainedTree, id: NodeId) {
        let prev = prev.as_any().downcast_ref::<Spacer>().expect("same type");
        if self.sizing != prev.sizing {
            tree.node_mut(id).kind = NodeKind::Spacer(self.sizing);
        }
    }

    fn teardown(&self, tree: &mut RetainedTree, id: NodeId) {
        tree.free(id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ───────────────────────────── Stack composite ─────────────────────────

/// A horizontal or vertical Flex-lite stack (§5/§11). Holds a heterogeneous,
/// variable-length child list reconciled by the `ElementSplice` cursor.
pub struct Stack {
    pub widget_id: Option<WidgetId>,
    pub style: StackStyle,
    pub children: Vec<BoxView>,
    /// `child_sizing[i]` is how child `i` sizes on the parent's main axis.
    pub child_sizing: Vec<Sizing>,
}

impl Stack {
    fn empty(axis: Axis) -> Self {
        Self {
            widget_id: None,
            style: StackStyle {
                axis,
                main: MainAxis::Start,
                cross: CrossAxis::Start,
                gap: 0.0,
                padding: Insets::default(),
            },
            children: Vec::new(),
            child_sizing: Vec::new(),
        }
    }

    pub fn hstack() -> Self {
        Self::empty(Axis::Horizontal)
    }

    pub fn vstack() -> Self {
        Self::empty(Axis::Vertical)
    }

    pub fn id(mut self, wid: WidgetId) -> Self {
        self.widget_id = Some(wid);
        self
    }

    pub fn gap(mut self, gap: f32) -> Self {
        self.style.gap = gap;
        self
    }

    pub fn padding(mut self, padding: Insets) -> Self {
        self.style.padding = padding;
        self
    }

    pub fn main(mut self, main: MainAxis) -> Self {
        self.style.main = main;
        self
    }

    pub fn cross(mut self, cross: CrossAxis) -> Self {
        self.style.cross = cross;
        self
    }

    /// Append a child laid out at its intrinsic main size (`Sizing::Auto`).
    pub fn child<E: Element>(mut self, child: E) -> Self {
        self.children.push(Box::new(child));
        self.child_sizing.push(Sizing::Auto);
        self
    }

    /// Append a child with an explicit main-axis sizing (Flex/Fill/Fixed).
    pub fn child_sized<E: Element>(mut self, child: E, sizing: Sizing) -> Self {
        self.children.push(Box::new(child));
        self.child_sizing.push(sizing);
        self
    }

    /// Append an already-boxed child (for dynamically assembled lists).
    pub fn child_boxed(mut self, child: BoxView, sizing: Sizing) -> Self {
        self.children.push(child);
        self.child_sizing.push(sizing);
        self
    }

    /// Append a spacer whose main-axis size is `sizing`. A `Spacer` always
    /// measures to zero intrinsic size; its slot in the parent comes entirely
    /// from `sizing` (a `Fixed` gap or a `Flex`/`Fill` share of leftover).
    pub fn spacer(mut self, sizing: Sizing) -> Self {
        self.children.push(Box::new(Spacer { sizing }));
        self.child_sizing.push(sizing);
        self
    }
}

impl Element for Stack {
    fn build(&self, tree: &mut RetainedTree) -> NodeId {
        let id = tree.alloc(NodeKind::Stack(self.style.clone()));
        if let Some(wid) = self.widget_id {
            tree.map_widget(wid, id);
        }
        let mut child_ids = Vec::with_capacity(self.children.len());
        for child in &self.children {
            child_ids.push(child.build(tree));
        }
        let node = tree.node_mut(id);
        node.children = child_ids;
        node.per_child_sizing = self.child_sizing.clone();
        id
    }

    fn reconcile(&self, prev: &dyn Element, tree: &mut RetainedTree, id: NodeId) {
        let prev = prev.as_any().downcast_ref::<Stack>().expect("same type");
        if self.style != prev.style {
            tree.node_mut(id).kind = NodeKind::Stack(self.style.clone());
        }
        if let Some(wid) = self.widget_id {
            tree.map_widget(wid, id);
        }
        // Diff the variable-length child list via the splice. Re-borrows the
        // arena per child internally (§14).
        let prev_ids = tree.take_children(id);
        let new_ids = reconcile_children(tree, &prev.children, &self.children, prev_ids);
        let node = tree.node_mut(id);
        node.children = new_ids;
        node.per_child_sizing = self.child_sizing.clone();
    }

    fn teardown(&self, tree: &mut RetainedTree, id: NodeId) {
        // Take the child ids out of the slot, free each child's subtree via its
        // typed teardown, then free this node directly. We do NOT restore the
        // (now-freed) child ids before freeing the parent — `free` only frees
        // the parent slot (it does not recurse), so leaving `children` empty
        // avoids the double-free that a restore + `free_subtree` re-walk would
        // depend on `is_live` to skip.
        let children = tree.take_children(id);
        for (child, &cid) in self.children.iter().zip(children.iter()) {
            child.teardown(tree, cid);
        }
        // Defensive: free any child slots the typed list did not cover (a count
        // mismatch). Each remaining cid is freed once via free_subtree; covered
        // children are already freed, so this is a no-op for them.
        for &cid in children.iter().skip(self.children.len()) {
            free_subtree(tree, cid);
        }
        tree.free(id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ───────────────────────────── Block composite ─────────────────────────

/// A styled container (background + border + padding) wrapping one child (§11).
pub struct Block {
    pub widget_id: Option<WidgetId>,
    pub style: BlockStyle,
    pub child: BoxView,
}

impl Block {
    pub fn new<E: Element>(style: BlockStyle, child: E) -> Self {
        Self { widget_id: None, style, child: Box::new(child) }
    }

    pub fn id(mut self, wid: WidgetId) -> Self {
        self.widget_id = Some(wid);
        self
    }
}

impl Element for Block {
    fn build(&self, tree: &mut RetainedTree) -> NodeId {
        let id = tree.alloc(NodeKind::Block(self.style.clone()));
        if let Some(wid) = self.widget_id {
            tree.map_widget(wid, id);
        }
        let child_id = self.child.build(tree);
        tree.node_mut(id).children = vec![child_id];
        id
    }

    fn reconcile(&self, prev: &dyn Element, tree: &mut RetainedTree, id: NodeId) {
        let prev = prev.as_any().downcast_ref::<Block>().expect("same type");
        if self.style != prev.style {
            tree.node_mut(id).kind = NodeKind::Block(self.style.clone());
        }
        if let Some(wid) = self.widget_id {
            tree.map_widget(wid, id);
        }
        // Single-child diff through the splice (handles type-change by
        // rebuild). Re-borrows the arena per child internally.
        let prev_ids = tree.take_children(id);
        let new_ids = reconcile_children(
            tree,
            std::slice::from_ref(&prev.child),
            std::slice::from_ref(&self.child),
            prev_ids,
        );
        tree.node_mut(id).children = new_ids;
    }

    fn teardown(&self, tree: &mut RetainedTree, id: NodeId) {
        // Free the single child's subtree, then free this node directly. The
        // child id is left out of the slot (see Stack::teardown) so freeing the
        // parent cannot double-free the already-freed child.
        let children = tree.take_children(id);
        if let Some(&cid) = children.first() {
            self.child.teardown(tree, cid);
        }
        tree.free(id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ───────────────────────────── Modified composite ──────────────────────

/// A modifier-decorated wrapper around one child — the Compose-style styling
/// node (background / border / corner / shadow / padding / margin via a
/// [`Modifier`] chain). Built with [`Modify::modify`] on any element.
pub struct Modified {
    pub widget_id: Option<WidgetId>,
    pub modifier: Modifier,
    pub child: BoxView,
}

impl Modified {
    pub fn new<E: Element>(modifier: Modifier, child: E) -> Self {
        Self { widget_id: None, modifier, child: Box::new(child) }
    }

    pub fn id(mut self, wid: WidgetId) -> Self {
        self.widget_id = Some(wid);
        self
    }
}

impl Element for Modified {
    fn build(&self, tree: &mut RetainedTree) -> NodeId {
        let id = tree.alloc(NodeKind::Modified(self.modifier.clone()));
        if let Some(wid) = self.widget_id {
            tree.map_widget(wid, id);
        }
        let child_id = self.child.build(tree);
        tree.node_mut(id).children = vec![child_id];
        id
    }

    fn reconcile(&self, prev: &dyn Element, tree: &mut RetainedTree, id: NodeId) {
        let prev = prev.as_any().downcast_ref::<Modified>().expect("same type");
        if self.modifier != prev.modifier {
            tree.node_mut(id).kind = NodeKind::Modified(self.modifier.clone());
        }
        if let Some(wid) = self.widget_id {
            tree.map_widget(wid, id);
        }
        let prev_ids = tree.take_children(id);
        let new_ids = reconcile_children(
            tree,
            std::slice::from_ref(&prev.child),
            std::slice::from_ref(&self.child),
            prev_ids,
        );
        tree.node_mut(id).children = new_ids;
    }

    fn teardown(&self, tree: &mut RetainedTree, id: NodeId) {
        let children = tree.take_children(id);
        if let Some(&cid) = children.first() {
            self.child.teardown(tree, cid);
        }
        tree.free(id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Extension: apply a [`Modifier`] chain to any element, wrapping it in a
/// [`Modified`] node. `text.modify(Modifier::new().background(c).padding(p))`.
pub trait Modify: Element + Sized {
    fn modify(self, modifier: Modifier) -> Modified {
        Modified::new(modifier, self)
    }
}

impl<E: Element> Modify for E {}

// ───────────────────────────── reconcile entry ─────────────────────────

/// Build a fresh tree from a root element, recording it as the arena root.
pub fn build_root<E: Element>(tree: &mut RetainedTree, root: &E) -> NodeId {
    let id = root.build(tree);
    tree.root = Some(id);
    id
}

/// Incrementally reconcile the retained tree's root against a new root element
/// of the same concrete type, applying only deltas (§4). Returns the root id.
pub fn reconcile_root<E: Element>(
    tree: &mut RetainedTree,
    root_id: NodeId,
    prev: &E,
    next: &E,
) -> NodeId {
    next.reconcile(prev as &dyn Element, tree, root_id);
    tree.root = Some(root_id);
    root_id
}
