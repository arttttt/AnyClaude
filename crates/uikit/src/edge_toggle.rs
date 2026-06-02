//! A small rounded "pill" toggle for a panel edge (collapse/expand), built on
//! term_ui modifiers. Domain-agnostic — reused by both panel managers (the right
//! teammates overlay and the left sessions sidebar).

use term_ui::{Insets, Modifier, Modify, Modified, Text, WidgetId};

/// Direction the pill's chevron points.
#[derive(Clone, Copy)]
pub enum Chevron {
    Left,
    Right,
    Up,
    Down,
}

impl Chevron {
    fn glyph(self) -> &'static str {
        match self {
            Chevron::Left => "‹",
            Chevron::Right => "›",
            Chevron::Up => "˄",
            Chevron::Down => "˅",
        }
    }
}

/// Colours for an [`edge_toggle`] pill.
#[derive(Clone, Copy)]
pub struct EdgeTogglePalette {
    pub background: [f32; 4],
    pub border: [f32; 4],
    /// Chevron colour — the host varies it to double as an activity indicator.
    pub glyph: [f32; 4],
}

/// A rounded capsule with a centered chevron, tagged with `widget_id` so the
/// host can hit-test clicks on it. Sized to the chevron plus symmetric padding;
/// the host places it (centered) on a panel's inner edge. Built entirely from
/// term_ui modifiers — `corner_radius` is set huge so the box is a full pill
/// (the shader clamps it to half the short side).
pub fn edge_toggle(
    facing: Chevron,
    palette: EdgeTogglePalette,
    font_size: f32,
    widget_id: WidgetId,
) -> Modified {
    Text::new(facing.glyph(), font_size, palette.glyph)
        .modify(
            Modifier::new()
                .corner_radius(999.0)
                .background(palette.background)
                .border(1.0, palette.border)
                .padding(Insets::symmetric(5.0, 6.0)),
        )
        .id(widget_id)
}
