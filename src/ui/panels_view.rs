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

use term_ui::{
    BoxView, CrossAxis, Insets, Modified, Modifier, Modify, Sizing, Spacer, Stack, Text, WidgetId,
};
use uikit::{edge_toggle, pager, Chevron, EdgeTogglePalette, PagerPalette};

use crate::ui::panel_manager::{Panel, PanelManager, RenderMode};

// ── panels palette (logical px / linear RGBA) ──
/// Opaque column background — slightly darker than the popup bg so the overlay
/// reads as a distinct surface floating over the terminal. `pub` so the
/// coordinator paints the same backdrop under the live grid pages.
pub const OVERLAY_BG: [f32; 4] = [0.06, 0.06, 0.08, 1.0];
/// The column frame + edge line + pill border.
const OVERLAY_BORDER: [f32; 4] = [0.25, 0.25, 0.27, 1.0];
/// The column frame when the overlay holds KEYBOARD focus — a bright focus ring
/// so it reads as "keystrokes go to the teammate here, not the main terminal".
const OVERLAY_BORDER_ACTIVE: [f32; 4] = [0.30, 0.60, 0.95, 1.0];
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
/// Height of the pager's bottom indicator strip (‹ dots ›). `pub` so the
/// coordinator reserves it when sizing the live grid pages.
pub const STRIP_H: f32 = 28.0;

/// Stable base id for the pager's hit-test ids (arrows + dots); distinct from
/// the toggle pill's id. `pub` so the coordinator can derive the same ids
/// (`uikit::pager_{prev,next,dot}_id`) to hit-test strip clicks.
pub fn pager_base_id() -> WidgetId {
    WidgetId::from_path(&[0x9A6E2])
}

/// The pager indicator palette: the current page dot bright, the others dim, the
/// arrows bright.
fn pager_palette() -> PagerPalette {
    PagerPalette { dot_current: TITLE_COLOR, dot_idle: SUBTITLE_COLOR, arrow: TITLE_COLOR }
}

/// Stable widget id for the toggle/indicator pill, resolved against the laid-out
/// tree so the coordinator can hit-test clicks on it (collapse/expand). The
/// panels view assigns no other WidgetIds, so the path just needs to be distinct
/// from the chrome's `session_widget_id`.
pub fn panel_toggle_widget_id() -> WidgetId {
    WidgetId::from_path(&[0x9A9E1])
}

/// Build the overlay view for `mgr`. `expanded` controls whether the content is
/// shown (collapsed renders just the faded edge band; the pill is a SEPARATE
/// tree so it stays opaque). `scroll` is the animated pager position (page
/// units) and `page_w` the viewport width the host allots. The returned
/// `Modified` is the column (opaque bg + frame + collapse `fade`); the host
/// measures it tight to the overlay rect and places it at the overlay origin.
pub fn panel_manager_view(
    mgr: &PanelManager,
    expanded: bool,
    scroll: f32,
    page_w: f32,
    fade: f32,
    input_focused: bool,
) -> Modified {
    let column = Modifier::new().background(OVERLAY_BG).border(1.0, OVERLAY_BORDER).alpha(fade);

    // Collapsed / mid-collapse: just the faded edge band, no content.
    if !expanded {
        return Stack::hstack()
            .cross(CrossAxis::Stretch)
            .spacer(Sizing::Fixed(mgr.policy().collapsed_width))
            .modify(column);
    }

    match mgr.policy().render {
        RenderMode::Pager => {
            // The live teammate grids are drawn by the coordinator (R5) into the
            // page slots; the pager here renders only the frame + dots strip, with
            // EMPTY page placeholders (so the dots count + slide positions exist)
            // and NO background fill — the grids provide the backdrop, and a fill
            // would draw over them (round-rects paint after the grid's rects).
            let current = mgr.focus_index().unwrap_or(0);
            let pages: Vec<BoxView> =
                (0..mgr.len()).map(|_| Box::new(Spacer::fill()) as BoxView).collect();
            // The frame turns into a focus ring when the keyboard is routed here.
            let frame = if input_focused { OVERLAY_BORDER_ACTIVE } else { OVERLAY_BORDER };
            pager(pages, current, scroll, page_w, STRIP_H, FONT_SIZE, pager_palette(), pager_base_id())
                .modify(Modifier::new().border(1.0, frame).alpha(fade))
        }
        RenderMode::Switcher => {
            // Left sessions sidebar (later): a stack of session cards. Scaffold —
            // not yet driven at runtime (only the right overlay is live).
            let mut stack = Stack::vstack().cross(CrossAxis::Stretch);
            for panel in mgr.panels() {
                let focused = mgr.focus() == Some(panel.id);
                stack = stack
                    .child_sized(panel_box(panel, focused), Sizing::Fixed(BOX_H))
                    .spacer(Sizing::Fixed(GAP));
            }
            stack
                .spacer(Sizing::Fill)
                .modify(Modifier::new().padding(Insets::all(CONTENT_PAD)))
                .modify(column)
        }
    }
}

/// The standalone collapse/expand pill (the shared `uikit::edge_toggle`),
/// rendered OUTSIDE the faded column so it stays opaque when the panel collapses.
/// The coordinator centres it on the divider line. The chevron points the way a
/// click moves the overlay (`›` collapse when expanded, `‹` expand when
/// collapsed); its colour is the activity indicator (green while a child runs).
/// Tagged for hit-testing.
pub fn pill_view(expanded: bool, active: bool) -> Modified {
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
