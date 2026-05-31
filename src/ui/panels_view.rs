//! Presenter for a [`PanelManager`] as a `term_ui` overlay view — the right
//! teammates overlay (and, later, the left sessions sidebar: same function,
//! branches on the manager's policy). The coordinator renders this into a THIRD
//! retained tree on top of the terminal grid, alongside the chrome (E.6) and the
//! popup (E.7) trees. The grid itself stays a direct `populate_panel` (R5); this
//! view owns only the panel FRAMES — the column background, the per-panel boxes
//! (border + title), and the inner-edge toggle/indicator pill.
//!
//! Built on term_ui MODIFIERS: the column, the per-panel boxes, and the toggle
//! pill are all `element.modify(..)` (rounded fills + borders via the round-rect
//! primitive). The pill is the shared `uikit::edge_toggle` widget, reused by both
//! panel managers. Milestone 1 renders placeholder panels (no live terminal).

use term_ui::{CrossAxis, Insets, Modified, Modifier, Modify, Sizing, Stack, Text, WidgetId};
use uikit::{edge_toggle, Chevron, EdgeTogglePalette};

use crate::ui::panel_manager::{Panel, PanelManager};

// ── panels palette (logical px / linear RGBA) ──
/// Opaque column background — slightly darker than the popup bg so the overlay
/// reads as a distinct surface floating over the terminal.
const OVERLAY_BG: [f32; 4] = [0.06, 0.06, 0.08, 1.0];
/// The column frame + edge line + pill border.
const OVERLAY_BORDER: [f32; 4] = [0.25, 0.25, 0.27, 1.0];
/// Per-panel placeholder box background.
const PANEL_BG: [f32; 4] = [0.11, 0.11, 0.13, 1.0];
/// Panel title (bright).
const TITLE_COLOR: [f32; 4] = [0.85, 0.85, 0.85, 1.0];
/// Panel subtitle / placeholder body (dim).
const SUBTITLE_COLOR: [f32; 4] = [0.45, 0.45, 0.5, 1.0];
/// Toggle pill background.
const TOGGLE_BG: [f32; 4] = [0.14, 0.14, 0.17, 1.0];
/// Indicator lit — a child process is running (green, mirrors the chrome flash).
const INDICATOR_ACTIVE: [f32; 4] = [0.4, 0.85, 0.4, 1.0];
/// Indicator idle — no running child (dim grey chevron).
const INDICATOR_IDLE: [f32; 4] = [0.5, 0.5, 0.55, 1.0];

// ── dimensions (logical px) ──
// The inner-edge strip width (the overlay's collapsed width + the toggle/drag
// band) is `Policy::collapsed_width`, so the model, hit-testing, and this view
// share one source.
const FONT_SIZE: f32 = 13.0;
const LINE_H: f32 = 20.0;
/// Height of one placeholder panel box.
const BOX_H: f32 = 72.0;
/// Gap between stacked panel boxes.
const GAP: f32 = 8.0;
/// Inset of the panel stack from the column edges.
const CONTENT_PAD: f32 = 10.0;
/// Corner radius of a panel box.
const PANEL_CORNER: f32 = 6.0;
/// `cosmic_text::Weight::BOLD.0` — panel titles.
const WEIGHT_BOLD: u16 = 700;

/// Stable widget id for the toggle/indicator pill, resolved against the laid-out
/// tree so the coordinator can hit-test clicks on it (collapse/expand). The
/// panels view assigns no other WidgetIds, so the path just needs to be distinct
/// from the chrome's `session_widget_id`.
pub fn panel_toggle_widget_id() -> WidgetId {
    WidgetId::from_path(&[0x9A9E1])
}

/// Build the overlay view for `mgr`. `expanded` controls whether the panel stack
/// is shown (collapsed renders just the edge strip + pill). The returned
/// `Modified` is the column: opaque bg + frame wrapping an hstack of [edge strip,
/// padded panel stack]. The coordinator measures it tight to the overlay rect
/// and places it at the overlay origin (positioned, not centred).
pub fn panel_manager_view(mgr: &PanelManager, expanded: bool) -> Modified {
    // Edge strip: the toggle/indicator pill centred both axes.
    let strip = Stack::vstack()
        .cross(CrossAxis::Center)
        .spacer(Sizing::Fill)
        .child(toggle_pill(expanded, mgr.any_active()))
        .spacer(Sizing::Fill);

    let mut row = Stack::hstack()
        .cross(CrossAxis::Stretch)
        .child_sized(strip, Sizing::Fixed(mgr.policy().collapsed_width));

    if expanded {
        let mut stack = Stack::vstack().cross(CrossAxis::Stretch);
        for panel in mgr.panels() {
            let focused = mgr.focus() == Some(panel.id);
            stack = stack
                .child_sized(panel_box(panel, focused), Sizing::Fixed(BOX_H))
                .spacer(Sizing::Fixed(GAP));
        }
        stack = stack.spacer(Sizing::Fill);
        // Inset the panel stack from the column edges.
        let padded = stack.modify(Modifier::new().padding(Insets::all(CONTENT_PAD)));
        row = row.child_sized(padded, Sizing::Fill);
    }

    row.modify(
        Modifier::new()
            .background(OVERLAY_BG)
            .border(1.0, OVERLAY_BORDER),
    )
}

/// The collapse/expand pill on the inner edge (the shared `uikit::edge_toggle`).
/// The chevron points the way the click moves the overlay (`›` collapse-right
/// when expanded, `‹` expand-left when collapsed); its colour is the activity
/// indicator (green while a child runs, dim otherwise). Tagged for hit-testing.
fn toggle_pill(expanded: bool, active: bool) -> Modified {
    let facing = if expanded { Chevron::Right } else { Chevron::Left };
    let glyph = if active { INDICATOR_ACTIVE } else { INDICATOR_IDLE };
    edge_toggle(
        facing,
        EdgeTogglePalette { background: TOGGLE_BG, border: OVERLAY_BORDER, glyph },
        FONT_SIZE,
        panel_toggle_widget_id(),
    )
}

/// One placeholder panel box: the agent accent as a rounded border (thicker when
/// focused), a bold title, and a dim placeholder body. The live terminal grid
/// replaces the body in a later milestone.
fn panel_box(panel: &Panel, focused: bool) -> Modified {
    let border_width = if focused { 2.0 } else { 1.0 };
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
        .spacer(Sizing::Fill)
        .modify(
            Modifier::new()
                .corner_radius(PANEL_CORNER)
                .background(PANEL_BG)
                .border(border_width, panel.accent)
                .padding(Insets::all(CONTENT_PAD)),
        )
}
