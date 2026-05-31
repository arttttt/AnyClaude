//! Modifiers (Compose-inspired): a composable, ordered chain of decorations
//! applied to any [`Element`](crate::view::Element) via `.modify(..)`, producing
//! a [`Modified`](crate::view::Modified) wrapper node.
//!
//! A [`Modifier`] is an ordered list of [`Mod`] ops. Order is significant, like
//! Compose: the FIRST op written is the OUTERMOST. Layout ops (padding/margin)
//! shrink the box as the chain descends; draw ops (background/border/corner/
//! shadow) emit their decoration at the box bounds AT THAT POINT in the chain —
//! so `padding().background()` paints the bg inside the padding, while
//! `background().padding()` paints it outside (the box-model order honoured by
//! the paint fold, see `paint::paint`).
//!
//! Scope note vs Compose (deliberate, not a crutch): the chain is folded by a
//! single wrapper node, not one node per op; size-affecting ops are the additive
//! insets (padding/margin/border), so MEASURE is order-independent (a sum) while
//! PAINT honours order. Size/offset/fill and per-subtree alpha are not modelled
//! yet (the Stack sizing system covers child sizing); they slot in later as more
//! `Mod` variants without changing the fold shape.

use crate::arena::BlockShadow;
use crate::geometry::Insets;

/// One decoration in a [`Modifier`] chain.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mod {
    // ── layout (shrink the box) ──
    /// Inner inset: shrinks the content box; the background/border still cover
    /// the pre-padding bounds when `padding` comes after them in the chain.
    Padding(Insets),
    /// Outer inset: transparent space around the box (background/border sit
    /// inside it).
    Margin(Insets),

    // ── draw (emit a decoration at the current box bounds) ──
    /// Rounded-rect fill at the current bounds with the current corner radius.
    Background([f32; 4]),
    /// Rounded-rect border ring (`width` px) at the current bounds + corner.
    Border { width: f32, color: [f32; 4] },
    /// Sets the corner radius for subsequent draw ops in the chain.
    CornerRadius(f32),
    /// Drop shadow under the current bounds.
    Shadow(BlockShadow),
}

/// An ordered chain of [`Mod`]s. Cheap value type (the diff key for reconcile).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Modifier {
    pub ops: Vec<Mod>,
}

impl Modifier {
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    fn push(mut self, op: Mod) -> Self {
        self.ops.push(op);
        self
    }

    pub fn padding(self, insets: Insets) -> Self {
        self.push(Mod::Padding(insets))
    }

    pub fn margin(self, insets: Insets) -> Self {
        self.push(Mod::Margin(insets))
    }

    pub fn background(self, color: [f32; 4]) -> Self {
        self.push(Mod::Background(color))
    }

    pub fn border(self, width: f32, color: [f32; 4]) -> Self {
        self.push(Mod::Border { width, color })
    }

    pub fn corner_radius(self, radius: f32) -> Self {
        self.push(Mod::CornerRadius(radius))
    }

    pub fn shadow(self, shadow: BlockShadow) -> Self {
        self.push(Mod::Shadow(shadow))
    }

    /// Total leading inset `(left, top)` and full inset `(horizontal, vertical)`
    /// reserved by the layout ops — used by measure/place. Order-independent
    /// because padding/margin/border are additive.
    pub(crate) fn box_insets(&self) -> BoxInsets {
        let mut acc = BoxInsets::default();
        for op in &self.ops {
            match op {
                Mod::Padding(i) | Mod::Margin(i) => {
                    acc.left += i.left;
                    acc.top += i.top;
                    acc.right += i.right;
                    acc.bottom += i.bottom;
                }
                Mod::Border { width, .. } => {
                    acc.left += width;
                    acc.top += width;
                    acc.right += width;
                    acc.bottom += width;
                }
                _ => {}
            }
        }
        acc
    }
}

/// Accumulated box-model insets (logical px) from a modifier's layout ops.
#[derive(Default, Clone, Copy)]
pub(crate) struct BoxInsets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl BoxInsets {
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}
