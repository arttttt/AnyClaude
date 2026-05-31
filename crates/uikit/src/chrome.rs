//! Header / footer chrome **bars** — the reusable, domain-agnostic shape of
//! the top and bottom strips. They compose `term_ui` `Stack`/`Text`/`Spacer`/
//! `Block` and know nothing of what the text *says*: the caller hands in
//! already-formatted [`Segment`]s (the binary's presenter owns the
//! "backend:/Reqs:/Session:" vocabulary and the version string).
//!
//! Both bars FILL whatever box the caller `place`s them in (the caller imposes
//! the strip height — e.g. 24px header, 22px footer — via the layout
//! constraint). A 1px hairline fences the bar against the content region:
//! `header_bar` fences its BOTTOM edge, `footer_bar` its TOP edge.

use term_ui::{CrossAxis, Insets, Modified, Modifier, Modify, Sizing, Stack, Text, WidgetId};

/// Thickness of the chrome / content fence line, in logical pixels.
const FENCE_PX: f32 = 1.0;

/// One pre-formatted run of chrome text: the string and its colour. The
/// presenter decides both (e.g. the Session run flips to a flash colour when a
/// copy just happened); this kit only lays it out and paints it. An optional
/// `widget_id` makes the run hit-testable — the consumer tags a clickable run
/// (e.g. the session id) and resolves its bounds from the laid-out tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub text: String,
    pub color: [f32; 4],
    pub widget_id: Option<WidgetId>,
}

impl Segment {
    pub fn new(text: impl Into<String>, color: [f32; 4]) -> Self {
        Self { text: text.into(), color, widget_id: None }
    }

    /// Tag this run with a stable `WidgetId` so the caller can hit-test it.
    pub fn id(mut self, widget_id: WidgetId) -> Self {
        self.widget_id = Some(widget_id);
        self
    }
}

/// A header chrome bar: `segments` laid out left-to-right, joined by
/// `separator` (drawn in `sep_color`), with a 1px `fence_color` hairline along
/// the BOTTOM edge.
///
/// `leading_pad` insets the segment row from the left edge. The row takes all
/// the height minus the fence (`Sizing::Fill`); the fence stretches to the full
/// width (`CrossAxis::Stretch`).
pub fn header_bar(
    segments: &[Segment],
    separator: &str,
    sep_color: [f32; 4],
    font_size: f32,
    leading_pad: f32,
    fence_color: [f32; 4],
) -> Stack {
    Stack::vstack()
        .cross(CrossAxis::Stretch)
        .child_sized(
            segment_row(segments, separator, sep_color, font_size, leading_pad),
            Sizing::Fill,
        )
        .child_sized(fence(fence_color), Sizing::Fixed(FENCE_PX))
}

/// A footer chrome bar: `left` segments flush-left, `right` segments
/// flush-right (a `Spacer::fill()` divides them), with a 1px `fence_color`
/// hairline along the TOP edge.
pub fn footer_bar(
    left: &[Segment],
    right: &[Segment],
    font_size: f32,
    fence_color: [f32; 4],
) -> Stack {
    Stack::vstack()
        .cross(CrossAxis::Stretch)
        .child_sized(fence(fence_color), Sizing::Fixed(FENCE_PX))
        .child_sized(footer_row(left, right, font_size), Sizing::Fill)
}

/// The header's content row: `Text(seg)` runs interleaved with `Text(sep)`
/// (N segments → N-1 separators), vertically centred, inset from the left.
fn segment_row(
    segments: &[Segment],
    separator: &str,
    sep_color: [f32; 4],
    font_size: f32,
    leading_pad: f32,
) -> Stack {
    let mut row = Stack::hstack()
        .cross(CrossAxis::Center)
        .padding(Insets { left: leading_pad, ..Insets::default() });
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            row = row.child(Text::new(separator, font_size, sep_color));
        }
        let mut text = Text::new(seg.text.clone(), font_size, seg.color);
        if let Some(wid) = seg.widget_id {
            text = text.id(wid);
        }
        row = row.child(text);
    }
    row
}

/// The footer's content row: left runs, a fill spacer, then right runs.
fn footer_row(left: &[Segment], right: &[Segment], font_size: f32) -> Stack {
    let mut row = Stack::hstack().cross(CrossAxis::Center);
    for seg in left {
        row = row.child(Text::new(seg.text.clone(), font_size, seg.color));
    }
    row = row.spacer(Sizing::Fill);
    for seg in right {
        row = row.child(Text::new(seg.text.clone(), font_size, seg.color));
    }
    row
}

/// A 1px-tall horizontal rule: an empty `Spacer` with a `color` background via a
/// modifier. The parent pins it to `Sizing::Fixed(FENCE_PX)` on the main axis and
/// `CrossAxis::Stretch` widens it to the full cross extent, so it paints as a
/// full-width hairline regardless of where it sits in the bar.
fn fence(color: [f32; 4]) -> Modified {
    term_ui::Spacer::fixed(0.0).modify(Modifier::new().background(color))
}
