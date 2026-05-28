//! Pure geometry types shared by layout, paint, and (Phase B+) hit-testing.
//! All coordinates are **logical pixels** (scale-factor-divided), matching
//! term_gpu's `RectInstance`/`GlyphInstance` convention.

use glam::Vec2;

/// A placed, sized rectangle. `origin` is the top-left; `size` is width/height.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Bounds {
    pub origin: Vec2,
    pub size: Vec2,
}

impl Bounds {
    pub fn new(origin: Vec2, size: Vec2) -> Self {
        Self { origin, size }
    }

    pub fn right(&self) -> f32 {
        self.origin.x + self.size.x
    }

    pub fn bottom(&self) -> f32 {
        self.origin.y + self.size.y
    }

    /// Point-in-rect test (half-open on the far edges), for Phase B hit-testing.
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.origin.x
            && p.y >= self.origin.y
            && p.x < self.right()
            && p.y < self.bottom()
    }
}

/// Layout constraint: a min/max size box the parent imposes on a child
/// (constraints-down). Sizes are logical pixels. `max` components may be
/// `f32::INFINITY` for an unbounded axis.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SizeConstraint {
    pub min: Vec2,
    pub max: Vec2,
}

impl SizeConstraint {
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// A fixed (tight) constraint: min == max == `size`.
    pub fn tight(size: Vec2) -> Self {
        Self { min: size, max: size }
    }

    /// A loose constraint: 0..=`max`.
    pub fn loose(max: Vec2) -> Self {
        Self { min: Vec2::ZERO, max }
    }

    /// Clamp a desired size into the constraint box (sizes-up clamp).
    pub fn apply(&self, size: Vec2) -> Vec2 {
        Vec2::new(
            size.x.clamp(self.min.x, self.max.x),
            size.y.clamp(self.min.y, self.max.y),
        )
    }

    /// The max extent along an axis (used to detect infinite main-axis).
    pub fn max_along(&self, axis: Axis) -> f32 {
        axis.major(self.max)
    }
}

/// Layout axis. `Horizontal` is the main axis for an `HStack`, `Vertical`
/// for a `VStack`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    /// The component of `v` along this axis (the "major" extent).
    pub fn major(self, v: Vec2) -> f32 {
        match self {
            Axis::Horizontal => v.x,
            Axis::Vertical => v.y,
        }
    }

    /// The component of `v` across this axis (the "minor" extent).
    pub fn minor(self, v: Vec2) -> f32 {
        match self {
            Axis::Horizontal => v.y,
            Axis::Vertical => v.x,
        }
    }

    /// Build a `Vec2` from a (major, minor) pair along this axis.
    pub fn pack(self, major: f32, minor: f32) -> Vec2 {
        match self {
            Axis::Horizontal => Vec2::new(major, minor),
            Axis::Vertical => Vec2::new(minor, major),
        }
    }
}

/// Main-axis distribution of free space among children (Flex-lite, §5).
/// `SpaceEvenly` is deliberately omitted (no consumer — YAGNI).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MainAxis {
    Start,
    Center,
    End,
    SpaceBetween,
}

/// Cross-axis alignment of each child within the stack's cross extent (§5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrossAxis {
    Start,
    Center,
    End,
    Stretch,
}

/// Per-child sizing along the parent's main axis (Flex-lite, §5).
///
/// - `Fixed` pins the main extent to an exact logical-pixel value.
/// - `Flex(weight)` takes a share of the leftover main space proportional
///   to `weight`.
/// - `Fill` is shorthand for `Flex(1.0)` (takes remaining space).
/// - `Auto` measures the child at its intrinsic size (the default).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sizing {
    Auto,
    Fixed(f32),
    Flex(f32),
    Fill,
}

impl Sizing {
    /// The flex weight this sizing contributes, or `None` if it is inflexible.
    pub fn flex_weight(self) -> Option<f32> {
        match self {
            Sizing::Flex(w) => Some(w.max(0.0)),
            Sizing::Fill => Some(1.0),
            Sizing::Auto | Sizing::Fixed(_) => None,
        }
    }
}

/// Symmetric/asymmetric padding insets (logical pixels).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Insets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Insets {
    pub fn all(v: f32) -> Self {
        Self { left: v, top: v, right: v, bottom: v }
    }

    pub fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            left: horizontal,
            right: horizontal,
            top: vertical,
            bottom: vertical,
        }
    }

    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }

    pub fn total(&self) -> Vec2 {
        Vec2::new(self.horizontal(), self.vertical())
    }

    pub fn top_left(&self) -> Vec2 {
        Vec2::new(self.left, self.top)
    }
}
