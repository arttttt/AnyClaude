//! `ElementSplice` — the variable-length child list diff (design §4).
//!
//! A single forward pass over the new child list, emitting the four cursor
//! operations against the retained slots:
//!
//! - **mutate** — a prev child at this position has the SAME concrete type as
//!   the new child: reuse its generational slot, recurse `reconcile`.
//! - **insert** — no reusable prev child (ran out, or a type change forced a
//!   delete): `build` a fresh slot for the new child.
//! - **delete** — a prev child at this position has a DIFFERENT concrete type:
//!   `teardown` its subtree (freeing slots, bumping generations so stale ids
//!   stop resolving) before inserting the replacement.
//! - **skip** — implicit: positions past the new list's end that have no prev
//!   child contribute nothing.
//!
//! Alignment is **positional** (index-aligned `prev[i]` vs `next[i]`), not
//! React-`key=` keyed. §4 frames the splice as a cursor whose alignment is by
//! stable `WidgetId` "not the list position"; in Phase A that stronger
//! key-aware alignment is deliberately deferred to its consumer (Phase B's
//! `view(&AppState)` + per-`WidgetId` AppState), per R13 — no abstraction
//! precedes its consumer. The identity that *survives* this positional splice
//! is still the stable `WidgetId`: each reused slot is re-stamped via the
//! element's `reconcile` → `map_widget`, and bucket-2 state keys on `WidgetId`,
//! never on the reused `NodeId` slot. The r4 reorder cases exercise exactly
//! that. The pass re-borrows the arena per child — it never holds an
//! `&mut Node` across a recursive `build`/`reconcile` call (§14).

use crate::arena::RetainedTree;
use crate::id::NodeId;
use crate::view::BoxView;

/// Reconcile a parent's children. `prev` is last frame's boxed children
/// (aligned with `prev_ids`); `next` is this frame's. Returns the new child id
/// list (one id per `next` element). Trailing prev children are torn down.
pub fn reconcile_children(
    tree: &mut RetainedTree,
    prev: &[BoxView],
    next: &[BoxView],
    prev_ids: Vec<NodeId>,
) -> Vec<NodeId> {
    let mut out = Vec::with_capacity(next.len());
    let common = prev.len().min(next.len());

    for i in 0..common {
        let prev_child = &prev[i];
        let next_child = &next[i];
        let slot = prev_ids[i];
        if prev_child.concrete_type() == next_child.concrete_type() && tree.is_live(slot) {
            // mutate: reuse the live slot, apply deltas.
            next_child.reconcile(prev_child.as_ref(), tree, slot);
            out.push(slot);
        } else {
            // delete (type change or stale slot) then insert.
            if tree.is_live(slot) {
                prev_child.teardown(tree, slot);
            }
            out.push(next_child.build(tree));
        }
    }

    // Trailing new children: insert.
    for next_child in next.iter().skip(common) {
        out.push(next_child.build(tree));
    }

    // Trailing prev children: delete. `prev[common..]` and `prev_ids[common..]`
    // are index-aligned, so each trailing prev box gets a typed teardown that
    // frees its whole subtree (no generic-free fallback is reachable here).
    for (prev_child, &slot) in prev.iter().zip(prev_ids.iter()).skip(common) {
        if tree.is_live(slot) {
            prev_child.teardown(tree, slot);
        }
    }

    out
}
