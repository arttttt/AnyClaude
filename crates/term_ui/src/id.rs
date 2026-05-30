//! The two identities of the engine (design §1 R8).
//!
//! - [`WidgetId`] is the **stable id-path** identity. It survives a full
//!   teardown+rebuild and any structural reorder, and is the ONLY identity
//!   that AppState (focus / fields / animations / hover, in Phase B+) would
//!   ever key on. Cheap `Copy`/`Eq`/`Hash`.
//! - [`NodeId`] is the **generational arena slot** identity — bucket-2
//!   internal only, never stored in AppState. It is ABA-safe: a freed slot
//!   that is later reused gets a bumped generation, so a stale `NodeId`
//!   (captured before the slot was recycled) does NOT resolve.
//!
//! Keeping these as two distinct types (rather than interchangeable
//! integers) makes the R8 boundary a type-level fact: you cannot
//! accidentally store an arena slot where a stable id is required.

/// Stable id-path identity (Xilem `ViewId`-style). Survives full
/// teardown+rebuild and reordering. The only identity that faces AppState.
///
/// Constructed from an id-path of small integers via [`WidgetId::from_path`]
/// so a widget's identity is derived from its position-by-key in the view
/// tree, not from an arena slot or a pointer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WidgetId(pub u64);

impl WidgetId {
    /// FNV-1a 64-bit. Deterministic, no allocation, good enough avalanche
    /// for id-path hashing (we are not defending against adversarial keys).
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    /// Derive a stable id from an id-path. Two widgets with the same path
    /// get the same `WidgetId` across frames; a reorder that changes a
    /// widget's key changes its id (so its retained state follows the key,
    /// not the slot).
    pub fn from_path(path: &[u64]) -> Self {
        let mut h = Self::FNV_OFFSET;
        for &seg in path {
            for b in seg.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(Self::FNV_PRIME);
            }
        }
        WidgetId(h)
    }

    /// Extend an existing id by one path segment (child key). Lets a parent
    /// hand each child a distinct stable id without rebuilding the whole
    /// path slice every time.
    pub fn child(self, seg: u64) -> Self {
        let mut h = self.0;
        for b in seg.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(Self::FNV_PRIME);
        }
        WidgetId(h)
    }
}

/// Generational arena slot identity (design §14). `idx` selects a slot in
/// `RetainedTree::nodes`; `generation` must match the slot's current
/// generation for the id to resolve. Bucket-2 internal only (R8).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId {
    pub(crate) idx: u32,
    pub(crate) generation: u32,
}

impl NodeId {
    pub fn index(self) -> usize {
        self.idx as usize
    }

    pub fn generation(self) -> u32 {
        self.generation
    }
}
