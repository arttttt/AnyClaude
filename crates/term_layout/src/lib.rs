//! BSP panel layout tree for AnyClaude's GPU terminal.
//!
//! A pure data-structure crate: produces rectangles, consumers decide
//! what to do with them. No rendering, no PTY, no UX hooks. See
//! `docs/gpu-terminal-spec.md` §6 for the surrounding design.
//!
//! Bootstrap commit: the public types and an empty tree with a single
//! full-window panel. `split` / `close` / `resize` / `hit_test` /
//! `drag_divider` land in subsequent commits.

/// Stable handle for a panel. Issued by [`PanelTree`] when a panel is
/// created (initial tree + every `split`) and never reused — closing a
/// panel does not free its id for reissue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelId(pub u64);

/// Axis-aligned rectangle in logical pixels. Anchored at the top-left
/// like the rest of the GPU stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Orientation of a split. `Horizontal` puts the divider line
/// horizontally (children stack vertically); `Vertical` puts the
/// divider vertically (children sit side-by-side).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Horizontal,
    Vertical,
}

/// Internal BSP node. Callers interact only via [`PanelTree`] and
/// [`PanelId`]; the tree shape is an implementation detail.
enum Node {
    Leaf {
        id: PanelId,
        bounds: Rect,
    },
    #[allow(dead_code)] // surfaces in the `split` commit
    Branch {
        split: Split,
        ratio: f32,
        bounds: Rect,
        left: Box<Node>,
        right: Box<Node>,
    },
}

/// The panel layout tree. Holds the root node and the currently
/// focused panel.
pub struct PanelTree {
    root: Option<Node>,
    focus: PanelId,
}

impl PanelTree {
    /// Create a tree with a single panel filling `(0, 0, w, h)`. The
    /// initial panel is focused.
    pub fn new(w: f32, h: f32) -> Self {
        let id = PanelId(0);
        let root = Node::Leaf {
            id,
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            },
        };
        Self {
            root: Some(root),
            focus: id,
        }
    }

    /// The currently focused panel.
    pub fn focus(&self) -> PanelId {
        self.focus
    }

    /// True if the tree contains no panels (every panel was closed).
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// All leaf panels with their current bounds, in depth-first
    /// left-then-right order. The order is stable across calls when
    /// the tree is unchanged but should not be relied on for hit
    /// testing — use [`PanelTree::hit_test`] for that (lands in a
    /// later commit).
    pub fn panels(&self) -> Vec<(PanelId, Rect)> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            collect_leaves(root, &mut out);
        }
        out
    }
}

fn collect_leaves(node: &Node, out: &mut Vec<(PanelId, Rect)>) {
    match node {
        Node::Leaf { id, bounds } => out.push((*id, *bounds)),
        Node::Branch { left, right, .. } => {
            collect_leaves(left, out);
            collect_leaves(right, out);
        }
    }
}
