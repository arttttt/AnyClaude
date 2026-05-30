//! The retained tree (bucket 2): a **flat generational arena** (design §14).
//!
//! No `Rc<RefCell>` / `Arc<Mutex>` anywhere. Children are `Vec<NodeId>`
//! (arena indices), so the tree is a single-owner `Vec`. Stale-slot safety
//! comes from a per-slot generation counter: freeing a slot bumps its
//! generation, so a [`NodeId`] captured before the free no longer resolves
//! (ABA-safe, R8).
//!
//! `measure` / `place` / `paint` / `rebuild` are index-based free functions
//! (in their own modules) that re-borrow the arena per child — they never
//! hold an `&mut Node` across a recursive child call (§14). The arena offers
//! small accessors (`kind`, `kind_mut`, `children_of`, `set_*`) that take
//! `&self`/`&mut self` for one slot at a time, plus `take_children` /
//! `restore_children` for the copy-out / recurse / put-back idiom.

use std::collections::HashMap;

use glam::Vec2;

use crate::geometry::{Bounds, CrossAxis, Insets, MainAxis, Sizing};
use crate::id::{NodeId, WidgetId};

/// What a node *is* — its paintable/laid-out content and style. This is the
/// declarative payload that a `View` writes into the slot; it is pure data
/// (no GPU handles), so the whole tree is `Clone` for the R4 property test's
/// rebuild-from-scratch comparison.
#[derive(Clone, PartialEq, Debug)]
pub enum NodeKind {
    /// A horizontal or vertical stack with Flex-lite layout config.
    Stack(StackStyle),
    /// A styled container (background + optional border) that wraps a single
    /// child with padding.
    Block(BlockStyle),
    /// A single line of variable-width text.
    Text(TextStyle),
    /// Empty space; sized by its `Sizing` in the parent (a flexible spacer)
    /// or a fixed gap. Paints nothing.
    Spacer(Sizing),
}

/// Flex-lite stack configuration (§5).
#[derive(Clone, PartialEq, Debug)]
pub struct StackStyle {
    pub axis: crate::geometry::Axis,
    pub main: MainAxis,
    pub cross: CrossAxis,
    pub gap: f32,
    pub padding: Insets,
}

/// Optional drop shadow under a [`Block`]'s background (design §11). Emitted as
/// one `term_gpu::ShadowInstance` sized to the Block's placed bounds: a rounded-
/// rect SDF halo drawn UNDER the bg rect, so the opaque bg covers the saturated
/// centre and only the soft halo shows. `None` (the default) emits nothing, so
/// existing chrome Blocks stay byte-identical and untouched.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BlockShadow {
    pub blur_radius: f32,
    pub corner_radius: f32,
    pub offset: [f32; 2],
    pub color: [f32; 4],
}

/// Block (container) style: background fill + optional border + padding, plus an
/// optional drop `shadow` (popups; `None` for plain containers).
#[derive(Clone, PartialEq, Debug)]
pub struct BlockStyle {
    pub background: [f32; 4],
    pub border_color: [f32; 4],
    pub border_width: f32,
    pub padding: Insets,
    pub shadow: Option<BlockShadow>,
}

/// Text content + shaping style.
#[derive(Clone, PartialEq, Debug)]
pub struct TextStyle {
    pub text: String,
    pub font_size: f32,
    /// term_gpu `Weight` is `cosmic_text::Weight(u16)` — stored as the raw
    /// `u16` here so `NodeKind` stays `PartialEq`/`Clone` without leaking
    /// the dependency into the arena's derives.
    pub weight: u16,
    /// `true` = italic, `false` = upright. (term_gpu `Style` has no third
    /// variant in our consumer set.)
    pub italic: bool,
    pub color: [f32; 4],
}

/// One arena slot. Identity lives only at boundaries (`widget_id`, §4); the
/// layout output (`measured`, `bounds`) and the child list live alongside the
/// declarative `kind`. `per_child_sizing[i]` is how child `i` sizes along the
/// parent's main axis (Flex-lite); empty/short ⇒ `Sizing::Auto`.
#[derive(Clone, PartialEq, Debug)]
pub struct Node {
    pub widget_id: Option<WidgetId>,
    pub kind: NodeKind,
    pub children: Vec<NodeId>,
    pub per_child_sizing: Vec<Sizing>,
    /// Bottom-up measured size (logical px). Set by `measure`.
    pub measured: Vec2,
    /// Top-down placed bounds (logical px). Set by `place`.
    pub bounds: Bounds,
    pub focusable: bool,
}

impl Node {
    fn new(kind: NodeKind) -> Self {
        Self {
            widget_id: None,
            kind,
            children: Vec::new(),
            per_child_sizing: Vec::new(),
            measured: Vec2::ZERO,
            bounds: Bounds::new(Vec2::ZERO, Vec2::ZERO),
            focusable: false,
        }
    }

    /// Sizing of child at position `i`, defaulting to `Auto`.
    pub fn child_sizing(&self, i: usize) -> Sizing {
        self.per_child_sizing.get(i).copied().unwrap_or(Sizing::Auto)
    }
}

/// A slot in the arena. `Free` slots are recycled via the free list; the
/// generation is bumped on free so a stale `NodeId` to this slot fails to
/// resolve (ABA-safe).
enum Slot {
    Live { node: Node, generation: u32 },
    Free { generation: u32 },
}

/// The retained tree — owned by the coordinator (Phase B+); in Phase A it is
/// owned by the toy/tests directly. A flat `Vec` of generational slots plus a
/// free list, a `WidgetId -> NodeId` map (rebuilt per reconcile), a dirty set,
/// and the derived `focus_order` (rebuilt per reconcile).
pub struct RetainedTree {
    slots: Vec<Slot>,
    free: Vec<u32>,
    id_map: HashMap<WidgetId, NodeId>,
    dirty: Vec<WidgetId>,
    pub root: Option<NodeId>,
    /// Focusable widget ids in depth-first tree order (bucket 2, rebuilt per
    /// reconcile). Stable `WidgetId`s so an item's identity survives reorder.
    pub focus_order: Vec<WidgetId>,
}

impl Default for RetainedTree {
    fn default() -> Self {
        Self::new()
    }
}

impl RetainedTree {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            id_map: HashMap::new(),
            dirty: Vec::new(),
            root: None,
            focus_order: Vec::new(),
        }
    }

    /// Allocate a slot for `node`, returning its generational id. Reuses a
    /// freed slot (keeping its bumped generation) when one is available.
    pub fn alloc(&mut self, kind: NodeKind) -> NodeId {
        let node = Node::new(kind);
        if let Some(idx) = self.free.pop() {
            let slot = &mut self.slots[idx as usize];
            let generation = match slot {
                Slot::Free { generation } => *generation,
                Slot::Live { generation, .. } => *generation,
            };
            *slot = Slot::Live { node, generation };
            NodeId { idx, generation }
        } else {
            let idx = self.slots.len() as u32;
            let generation = 0;
            self.slots.push(Slot::Live { node, generation });
            NodeId { idx, generation }
        }
    }

    /// Free a slot, bumping its generation so any stale `NodeId` referring to
    /// it stops resolving. Does NOT recursively free children — callers walk
    /// the subtree (teardown) and free bottom-up.
    pub fn free(&mut self, id: NodeId) {
        let Some(idx) = self.live_index(id) else {
            return;
        };
        if let Some(wid) = self.slots[idx].widget_id() {
            // Drop the map entry only if it still points at this exact slot.
            if self.id_map.get(&wid) == Some(&id) {
                self.id_map.remove(&wid);
            }
        }
        // Backstop against a double-free / free-list corruption: a live slot
        // should never already be on the free list. (The `live_index` guard
        // above already returns early for an already-freed slot, so reaching
        // here with `idx` on the free list would mean a generation collision.)
        debug_assert!(
            !self.free.contains(&id.idx),
            "term_ui: slot idx already on the free list (double-free)"
        );
        let next_gen = id.generation.wrapping_add(1);
        self.slots[idx] = Slot::Free { generation: next_gen };
        self.free.push(id.idx);
    }

    /// Resolve `id` to its slot index iff it is live AND the generation
    /// matches (ABA-safe). Returns `None` for a stale or out-of-range id.
    fn live_index(&self, id: NodeId) -> Option<usize> {
        let idx = id.idx as usize;
        match self.slots.get(idx) {
            Some(Slot::Live { generation, .. }) if *generation == id.generation => Some(idx),
            _ => None,
        }
    }

    /// `true` iff `id` resolves to a live slot with matching generation.
    pub fn is_live(&self, id: NodeId) -> bool {
        self.live_index(id).is_some()
    }

    /// Immutable access to a slot's node (None if stale/freed).
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.live_index(id).map(|i| self.slots[i].node_ref())
    }

    /// Mutable access to a slot's node (None if stale/freed).
    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        match self.live_index(id) {
            Some(i) => Some(self.slots[i].node_mut()),
            None => None,
        }
    }

    /// Borrow the node, panicking if stale — for internal index-based passes
    /// that have already validated the id this frame.
    pub fn node(&self, id: NodeId) -> &Node {
        self.get(id).expect("term_ui: NodeId resolved to a freed/stale slot")
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        self.get_mut(id)
            .expect("term_ui: NodeId resolved to a freed/stale slot")
    }

    /// Copy out a node's child list, leaving the slot's `children` empty.
    /// Use with [`restore_children`](Self::restore_children) for the
    /// per-child re-borrow recursion idiom (§14).
    pub fn take_children(&mut self, id: NodeId) -> Vec<NodeId> {
        std::mem::take(&mut self.node_mut(id).children)
    }

    pub fn restore_children(&mut self, id: NodeId, children: Vec<NodeId>) {
        self.node_mut(id).children = children;
    }

    /// Map a stable `WidgetId` to its current arena slot (this reconcile).
    pub fn resolve_widget(&self, wid: WidgetId) -> Option<NodeId> {
        self.id_map.get(&wid).copied()
    }

    /// Record (or update) the `WidgetId -> NodeId` mapping and stamp the slot.
    pub fn map_widget(&mut self, wid: WidgetId, id: NodeId) {
        self.node_mut(id).widget_id = Some(wid);
        self.id_map.insert(wid, id);
    }

    /// Mark a widget dirty (a controlled edit touched its subtree). Realized
    /// as a push to the dirty set rather than a whole-arena `NodeMut` (§14).
    pub fn mark_dirty(&mut self, wid: WidgetId) {
        if !self.dirty.contains(&wid) {
            self.dirty.push(wid);
        }
    }

    pub fn dirty(&self) -> &[WidgetId] {
        &self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty.clear();
    }

    /// Number of currently-live slots (for tests / leak checks).
    pub fn live_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_live()).count()
    }

    /// Total slot capacity including free slots (for tests).
    pub fn slot_capacity(&self) -> usize {
        self.slots.len()
    }
}

impl Slot {
    fn is_live(&self) -> bool {
        matches!(self, Slot::Live { .. })
    }

    fn node_ref(&self) -> &Node {
        match self {
            Slot::Live { node, .. } => node,
            Slot::Free { .. } => unreachable!("node_ref on a free slot"),
        }
    }

    fn node_mut(&mut self) -> &mut Node {
        match self {
            Slot::Live { node, .. } => node,
            Slot::Free { .. } => unreachable!("node_mut on a free slot"),
        }
    }

    fn widget_id(&self) -> Option<WidgetId> {
        match self {
            Slot::Live { node, .. } => node.widget_id,
            Slot::Free { .. } => None,
        }
    }
}
