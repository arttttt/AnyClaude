//! BSP panel layout tree for AnyClaude's GPU terminal.
//!
//! A pure data-structure crate: produces rectangles, consumers decide
//! what to do with them. No rendering, no PTY, no UX hooks. See
//! `docs/gpu-terminal-spec.md` §6 for the surrounding design.
//!
//! `split` / `resize` / `close` / `hit_test` are in place; the only
//! remaining mutator is `drag_divider`, which lands in the next commit.

/// Minimum and maximum split ratios. Splits clamp the requested ratio
/// into this range to avoid degenerate zero-area panels.
pub const MIN_RATIO: f32 = 0.05;
pub const MAX_RATIO: f32 = 0.95;

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
    Branch {
        split: Split,
        ratio: f32,
        bounds: Rect,
        left: Box<Node>,
        right: Box<Node>,
    },
}

/// The panel layout tree. Holds the root node, the next id to issue,
/// and the currently focused panel.
pub struct PanelTree {
    root: Option<Node>,
    next_id: u64,
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
            next_id: 1,
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

    /// Split `target` into two panels. The original keeps its id and
    /// becomes the left/top child; the new panel takes the right/bottom
    /// half. `ratio` is clamped to `[MIN_RATIO, MAX_RATIO]` to avoid
    /// degenerate zero-area panels. Returns the new panel's id, or
    /// `None` if `target` is not in the tree.
    ///
    /// On success the new panel becomes focused — matching the
    /// "open a split and start typing into it" behaviour familiar from
    /// Warp / tmux.
    pub fn split(&mut self, target: PanelId, split: Split, ratio: f32) -> Option<PanelId> {
        let ratio = ratio.clamp(MIN_RATIO, MAX_RATIO);
        let new_id = PanelId(self.next_id);
        let happened = match self.root.as_mut() {
            Some(root) => split_node(root, target, split, ratio, new_id),
            None => false,
        };
        if happened {
            self.next_id += 1;
            self.focus = new_id;
            Some(new_id)
        } else {
            None
        }
    }

    /// Reflow the tree to a new window size. Every leaf's bounds are
    /// recomputed by walking the tree top-down: each branch redivides
    /// its (new) bounds using its stored `ratio`, so all splits keep
    /// their proportions across resizes.
    pub fn resize(&mut self, w: f32, h: f32) {
        if let Some(root) = self.root.as_mut() {
            let new_bounds = Rect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            };
            recompute_bounds(root, new_bounds);
        }
    }

    /// Find the panel containing the point `(x, y)`. Returns `None`
    /// when the point falls outside every panel's bounds (most often
    /// because the tree is empty, but also when the point sits exactly
    /// on the right or bottom edge — bounds are half-open on those
    /// edges to avoid two panels claiming the same divider pixel).
    pub fn hit_test(&self, x: f32, y: f32) -> Option<PanelId> {
        self.root.as_ref().and_then(|root| hit_test_node(root, x, y))
    }

    /// Remove a panel. The sibling (or sibling subtree) absorbs the
    /// parent branch's bounds; the tree never keeps a one-child Branch
    /// around. Closing the only remaining panel empties the tree —
    /// the calling code is expected to react to `is_empty()` (e.g. by
    /// closing the window).
    ///
    /// Focus moves to the first panel still present (depth-first first
    /// leaf) when the focused panel was the one that closed.
    pub fn close(&mut self, target: PanelId) {
        let old_root = self.root.take();
        self.root = old_root.and_then(|node| close_node(node, target));
        if self.focus == target {
            self.focus = self
                .panels()
                .first()
                .map(|(id, _)| *id)
                .unwrap_or(self.focus);
        }
    }
}

/// Recursively remove `target` from this subtree. Returns the subtree's
/// new root, or `None` if the entire subtree collapsed (every leaf in
/// it was `target` — in practice only ever the immediate target leaf).
/// When a `Branch` loses one of its children, the survivor is promoted
/// to take the Branch's place and reflowed to fill the parent bounds.
fn close_node(node: Node, target: PanelId) -> Option<Node> {
    match node {
        Node::Leaf { id, .. } if id == target => None,
        leaf @ Node::Leaf { .. } => Some(leaf),
        Node::Branch {
            split,
            ratio,
            bounds,
            left,
            right,
        } => {
            let new_left = close_node(*left, target);
            let new_right = close_node(*right, target);
            match (new_left, new_right) {
                (None, None) => None,
                (Some(only), None) | (None, Some(only)) => {
                    let mut survivor = only;
                    recompute_bounds(&mut survivor, bounds);
                    Some(survivor)
                }
                (Some(l), Some(r)) => Some(Node::Branch {
                    split,
                    ratio,
                    bounds,
                    left: Box::new(l),
                    right: Box::new(r),
                }),
            }
        }
    }
}

fn split_node(
    node: &mut Node,
    target: PanelId,
    split: Split,
    ratio: f32,
    new_id: PanelId,
) -> bool {
    match node {
        Node::Leaf { id, bounds } if *id == target => {
            let parent_bounds = *bounds;
            let old_id = *id;
            let (left_bounds, right_bounds) = split_bounds(parent_bounds, split, ratio);
            *node = Node::Branch {
                split,
                ratio,
                bounds: parent_bounds,
                left: Box::new(Node::Leaf {
                    id: old_id,
                    bounds: left_bounds,
                }),
                right: Box::new(Node::Leaf {
                    id: new_id,
                    bounds: right_bounds,
                }),
            };
            true
        }
        Node::Leaf { .. } => false,
        Node::Branch { left, right, .. } => {
            split_node(left, target, split, ratio, new_id)
                || split_node(right, target, split, ratio, new_id)
        }
    }
}

fn hit_test_node(node: &Node, x: f32, y: f32) -> Option<PanelId> {
    match node {
        Node::Leaf { id, bounds } => {
            if rect_contains(*bounds, x, y) {
                Some(*id)
            } else {
                None
            }
        }
        Node::Branch { left, right, .. } => {
            hit_test_node(left, x, y).or_else(|| hit_test_node(right, x, y))
        }
    }
}

fn rect_contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

fn recompute_bounds(node: &mut Node, new_bounds: Rect) {
    match node {
        Node::Leaf { bounds, .. } => *bounds = new_bounds,
        Node::Branch {
            split,
            ratio,
            bounds,
            left,
            right,
        } => {
            *bounds = new_bounds;
            let (lb, rb) = split_bounds(new_bounds, *split, *ratio);
            recompute_bounds(left, lb);
            recompute_bounds(right, rb);
        }
    }
}

fn split_bounds(bounds: Rect, split: Split, ratio: f32) -> (Rect, Rect) {
    match split {
        Split::Horizontal => {
            let top_h = bounds.h * ratio;
            (
                Rect { h: top_h, ..bounds },
                Rect {
                    y: bounds.y + top_h,
                    h: bounds.h - top_h,
                    ..bounds
                },
            )
        }
        Split::Vertical => {
            let left_w = bounds.w * ratio;
            (
                Rect { w: left_w, ..bounds },
                Rect {
                    x: bounds.x + left_w,
                    w: bounds.w - left_w,
                    ..bounds
                },
            )
        }
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
