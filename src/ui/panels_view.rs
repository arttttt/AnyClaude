//! Presenter for a [`PanelManager`] as a `term_ui` overlay view — the right
//! teammates overlay (and, later, the left sessions sidebar: same function,
//! branches on the manager's policy). The coordinator renders this into a THIRD
//! retained tree on top of the terminal grid, alongside the chrome (E.6) and the
//! popup (E.7) trees. The grid itself stays a direct `populate_panel` (R5); this
//! view owns only the panel FRAMES — the column background, the per-panel boxes
//! (border + title), and the inner-edge toggle/indicator button.
//!
//! Milestone 1 renders placeholder panels (no live terminal inside). The frames,
//! titles, accents, focus highlight, and the indicator are exercised here; the
//! teammate's actual emulator grid lands with a later milestone.

use term_ui::{Block, BlockStyle, CrossAxis, Insets, Sizing, Stack, Text, WidgetId};

use crate::ui::panel_manager::{Panel, PanelManager};

// ── panels palette (logical px / linear RGBA) ──
/// Opaque column background — slightly darker than the popup bg so the overlay
/// reads as a distinct surface floating over the terminal.
const OVERLAY_BG: [f32; 4] = [0.06, 0.06, 0.08, 1.0];
/// The column frame + edge line + toggle button border.
const OVERLAY_BORDER: [f32; 4] = [0.25, 0.25, 0.27, 1.0];
/// Per-panel placeholder box background.
const PANEL_BG: [f32; 4] = [0.11, 0.11, 0.13, 1.0];
/// Panel title (bright).
const TITLE_COLOR: [f32; 4] = [0.85, 0.85, 0.85, 1.0];
/// Panel subtitle / placeholder body (dim).
const SUBTITLE_COLOR: [f32; 4] = [0.45, 0.45, 0.5, 1.0];
/// Toggle button background.
const TOGGLE_BG: [f32; 4] = [0.14, 0.14, 0.17, 1.0];
/// Indicator lit — a child process is running (green, mirrors the chrome flash).
const INDICATOR_ACTIVE: [f32; 4] = [0.4, 0.85, 0.4, 1.0];
/// Indicator idle — no running child (dim grey chevron).
const INDICATOR_IDLE: [f32; 4] = [0.5, 0.5, 0.55, 1.0];

// ── dimensions (logical px) ──
/// Width of the inner-edge strip — the overlay's collapsed width, and the
/// horizontal band the toggle button + (later) the drag handle occupy.
pub const PANEL_EDGE_STRIP_W: f32 = 20.0;
/// Height of the vertically-centered toggle/indicator button.
const TOGGLE_BTN_H: f32 = 52.0;
const FONT_SIZE: f32 = 13.0;
const LINE_H: f32 = 20.0;
/// Height of one placeholder panel box.
const BOX_H: f32 = 72.0;
/// Gap between stacked panel boxes.
const GAP: f32 = 8.0;
/// Inset of the panel stack from the column edges.
const CONTENT_PAD: f32 = 10.0;
/// `cosmic_text::Weight::BOLD.0` — panel titles.
const WEIGHT_BOLD: u16 = 700;

/// Stable widget id for the toggle/indicator button, resolved against the
/// laid-out tree so the coordinator can hit-test clicks on it (collapse/expand).
/// The panels view assigns no other WidgetIds, so the path just needs to be
/// distinct from the chrome's `session_widget_id`.
pub fn panel_toggle_widget_id() -> WidgetId {
    WidgetId::from_path(&[0x9A9E1])
}

/// Build the overlay view for `mgr`. `expanded` controls whether the panel stack
/// is shown (collapsed renders just the edge strip + button). The returned
/// `Block` is the column: opaque bg + frame wrapping an hstack of [edge strip,
/// padded panel stack]. The coordinator measures it tight to the overlay rect
/// and places it at the overlay origin (it is positioned, not centred).
pub fn panel_manager_view(mgr: &PanelManager, expanded: bool) -> Block {
    // Edge strip: the toggle/indicator button centred vertically.
    let strip = Stack::vstack()
        .cross(CrossAxis::Stretch)
        .spacer(Sizing::Fill)
        .child_sized(toggle_button(expanded, mgr.any_active()), Sizing::Fixed(TOGGLE_BTN_H))
        .spacer(Sizing::Fill);

    let mut row = Stack::hstack()
        .cross(CrossAxis::Stretch)
        .child_sized(strip, Sizing::Fixed(PANEL_EDGE_STRIP_W));

    if expanded {
        let mut stack = Stack::vstack().cross(CrossAxis::Stretch);
        for panel in mgr.panels() {
            let focused = mgr.focus() == Some(panel.id);
            stack = stack
                .child_sized(panel_box(panel, focused), Sizing::Fixed(BOX_H))
                .spacer(Sizing::Fixed(GAP));
        }
        stack = stack.spacer(Sizing::Fill);
        // Inset the panel stack from the column edges (padding-only Block).
        let padded = Block::new(transparent_padded(CONTENT_PAD), stack);
        row = row.child_sized(padded, Sizing::Fill);
    }

    Block::new(
        BlockStyle {
            background: OVERLAY_BG,
            border_color: OVERLAY_BORDER,
            border_width: 1.0,
            padding: Insets::default(),
            shadow: None,
        },
        row,
    )
}

/// The collapse/expand button on the inner edge. The chevron points the way the
/// click moves the overlay (`›` collapse-right when expanded, `‹` expand-left
/// when collapsed); its colour is the activity indicator (green while a child
/// runs, dim otherwise). Tagged so the coordinator can hit-test it.
fn toggle_button(expanded: bool, active: bool) -> Block {
    let glyph = if expanded { "›" } else { "‹" };
    let color = if active { INDICATOR_ACTIVE } else { INDICATOR_IDLE };
    Block::new(
        BlockStyle {
            background: TOGGLE_BG,
            border_color: OVERLAY_BORDER,
            border_width: 1.0,
            padding: Insets::default(),
            shadow: None,
        },
        Stack::vstack()
            .cross(CrossAxis::Stretch)
            .spacer(Sizing::Fill)
            .child_sized(Text::new(glyph, FONT_SIZE, color), Sizing::Fixed(LINE_H))
            .spacer(Sizing::Fill),
    )
    .id(panel_toggle_widget_id())
}

/// One placeholder panel box: the agent accent as a border (thicker when
/// focused), a bold title, and a dim placeholder body. The live terminal grid
/// replaces the body in a later milestone.
fn panel_box(panel: &Panel, focused: bool) -> Block {
    let border_width = if focused { 2.0 } else { 1.0 };
    Block::new(
        BlockStyle {
            background: PANEL_BG,
            border_color: panel.accent,
            border_width,
            padding: Insets::all(CONTENT_PAD),
            shadow: None,
        },
        Stack::vstack()
            .cross(CrossAxis::Stretch)
            .child_sized(
                Text::new(panel.title.as_str(), FONT_SIZE, TITLE_COLOR).weight(WEIGHT_BOLD),
                Sizing::Fixed(LINE_H),
            )
            .child_sized(
                Text::new("teammate", FONT_SIZE - 1.0, SUBTITLE_COLOR),
                Sizing::Fixed(LINE_H),
            )
            .spacer(Sizing::Fill),
    )
}

/// A transparent, borderless, shadowless Block style with uniform padding — used
/// only to inset a child from its parent's edges.
fn transparent_padded(pad: f32) -> BlockStyle {
    BlockStyle {
        background: [0.0; 4],
        border_color: [0.0; 4],
        border_width: 0.0,
        padding: Insets::all(pad),
        shadow: None,
    }
}
