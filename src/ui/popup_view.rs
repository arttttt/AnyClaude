//! Popup presenter: maps the popup stores (history / settings / backend switch)
//! into `term_ui` overlay views, which the coordinator renders into a SECOND
//! retained tree on top of the chrome (E.7). It mirrors the words, colours, and
//! row geometry of the legacy immediate-mode `gpu::popup` draw fns; the two
//! converge when `gpu/popup.rs` is deleted at the end of the popup port (E.7.8).
//!
//! Each popup is a [`popup_box`] — an opaque background, a 1px border, a drop
//! shadow, and uniform padding — wrapping a vertical body. The coordinator
//! measures the box under a [`POPUP_MIN_WIDTH`] floor and centres it with
//! `term_ui::place_centered`, so the per-popup width + centring math the old
//! code did by hand falls out of the layout pass instead.

use term_ui::{Block, BlockShadow, BlockStyle, CrossAxis, Insets, Sizing, Stack, Text};
use uikit::{fixed_row_window, popup_list, Segment};

use crate::config::SettingsFieldSnapshot;
use crate::ui::app_state::AppState;
use crate::ui::history::{HistoryEntry, MAX_VISIBLE_ROWS};
use crate::ui::settings::SettingsDialogState;

// ── popup palette (logical px / linear RGBA). Mirrors `gpu::popup` +
//    `gpu::chrome` until `gpu/popup.rs` is deleted (E.7.8), at which point these
//    become the single home. ──
const POPUP_BG_COLOR: [f32; 4] = [0.12, 0.12, 0.14, 1.0];
const POPUP_HIGHLIGHT_COLOR: [f32; 4] = [0.22, 0.30, 0.42, 1.0];
const POPUP_BORDER_COLOR: [f32; 4] = [0.30, 0.30, 0.35, 1.0];
const POPUP_SHADOW_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.45];
const POPUP_SHADOW_BLUR: f32 = 24.0;
const POPUP_SHADOW_OFFSET_Y: f32 = 8.0;
const POPUP_CORNER_RADIUS: f32 = 6.0;
const POPUP_LINE_HEIGHT: f32 = 22.0;
const POPUP_FONT_SIZE: f32 = 13.0;
const POPUP_PADDING: f32 = 12.0;

/// Floor for the popup box width (logical px): the coordinator passes this as
/// the measure constraint's min width so a short list never renders as a thin
/// sliver. (Replaces the old `content_w.max(POPUP_MIN_WIDTH)` clamp.)
pub const POPUP_MIN_WIDTH: f32 = 280.0;

/// Dim grey for unselected popup text + titles (mirrors `chrome::CHROME_TEXT_COLOR`).
const POPUP_TEXT_COLOR: [f32; 4] = [0.55, 0.55, 0.55, 1.0];
/// Brighter foreground for the selected row (contrasts the highlight bar).
const POPUP_SELECTED_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 1.0];

/// `cosmic_text::Weight::BOLD.0` — titles + section headers.
const WEIGHT_BOLD: u16 = 700;

/// The popup view for whichever popup is open (they are mutually exclusive), or
/// `None` when none is. The coordinator builds/reconciles this into the popup
/// tree, measures it (min-width floored), centres it, and paints it into the
/// overlay on top of the chrome.
///
/// Only the history popup is ported so far; backend switch + settings still
/// render via the immediate-mode `gpu::popup` path (later E.7 steps).
pub fn popup_view(state: &AppState) -> Option<Block> {
    if let crate::ui::history::HistoryDialogState::Visible { entries, scroll_offset } =
        &state.history
    {
        return Some(history_view(entries, *scroll_offset));
    }
    if let SettingsDialogState::Visible { fields, focused, .. } = &state.settings {
        return Some(settings_view(fields, *focused));
    }
    None
}

/// Wrap a popup `body` in the standard box: opaque bg, 1px border, drop shadow,
/// and uniform padding. This `Block` is the popup tree's root (centred as one).
fn popup_box(body: Stack) -> Block {
    Block::new(
        BlockStyle {
            background: POPUP_BG_COLOR,
            border_color: POPUP_BORDER_COLOR,
            border_width: 1.0,
            padding: Insets::all(POPUP_PADDING),
            shadow: Some(BlockShadow {
                blur_radius: POPUP_SHADOW_BLUR,
                corner_radius: POPUP_CORNER_RADIUS,
                offset: [0.0, POPUP_SHADOW_OFFSET_Y],
                color: POPUP_SHADOW_COLOR,
            }),
        },
        body,
    )
}

/// A BOLD title row, one line tall, in `color` (dim for the list popups, bright
/// for the backend switch).
fn title_row(title: &str, color: [f32; 4]) -> Text {
    Text::new(title, POPUP_FONT_SIZE, color).weight(WEIGHT_BOLD)
}

/// History popup: a read-only, newest-first list of backend switches, windowed
/// to `MAX_VISIBLE_ROWS` rows driven by `scroll_offset` (R11 virtualization —
/// the legacy immediate path drew every row and overflowed a tall history off
/// the window). Row strings + colours match the old popup verbatim.
fn history_view(entries: &[HistoryEntry], scroll_offset: usize) -> Block {
    let items: Vec<String> = entries
        .iter()
        .rev()
        .map(|e| {
            let secs = e
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let from = e.from_backend.as_deref().unwrap_or("(initial)");
            format!("{secs}  ·  {from}  →  {}", e.to_backend)
        })
        .collect();

    let mut body = Stack::vstack()
        .cross(CrossAxis::Stretch)
        .child_sized(title_row("History", POPUP_TEXT_COLOR), Sizing::Fixed(POPUP_LINE_HEIGHT))
        .spacer(Sizing::Fixed(POPUP_LINE_HEIGHT * 0.5));

    if items.is_empty() {
        body = body.child_sized(
            Text::new("(no history yet)", POPUP_FONT_SIZE, POPUP_TEXT_COLOR).italic(true),
            Sizing::Fixed(POPUP_LINE_HEIGHT),
        );
        return popup_box(body);
    }

    // Render only the visible row window; the highlight tracks the scroll cursor
    // (the row at `scroll_offset`), made relative to the window's first row.
    let window = fixed_row_window(scroll_offset, items.len(), MAX_VISIBLE_ROWS);
    let selected_rel = scroll_offset
        .min(items.len().saturating_sub(1))
        .saturating_sub(window.start);
    let rows: Vec<Segment> = items[window]
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let color = if i == selected_rel { POPUP_SELECTED_COLOR } else { POPUP_TEXT_COLOR };
            Segment::new(s.clone(), color)
        })
        .collect();
    body = body.child(popup_list(
        &rows,
        selected_rel,
        POPUP_LINE_HEIGHT,
        POPUP_HIGHLIGHT_COLOR,
        POPUP_FONT_SIZE,
    ));
    popup_box(body)
}

/// Settings popup: the title doubles as the hint line, then one toggle row per
/// field formatted `"[x]  {label}"` / `"[ ]  {label}"` (the checkbox is a glyph
/// prefix, exactly as the legacy popup), with the focused row highlighted.
/// Settings lists are short, so there is no virtualization (YAGNI).
fn settings_view(fields: &[SettingsFieldSnapshot], focused: usize) -> Block {
    let rows: Vec<Segment> = fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mark = if f.value { "[x]" } else { "[ ]" };
            let color = if i == focused { POPUP_SELECTED_COLOR } else { POPUP_TEXT_COLOR };
            Segment::new(format!("{mark}  {}", f.label), color)
        })
        .collect();
    let body = Stack::vstack()
        .cross(CrossAxis::Stretch)
        .child_sized(
            title_row("Settings  ·  Space toggle · Enter save · Esc cancel", POPUP_TEXT_COLOR),
            Sizing::Fixed(POPUP_LINE_HEIGHT),
        )
        .spacer(Sizing::Fixed(POPUP_LINE_HEIGHT * 0.5))
        .child(popup_list(&rows, focused, POPUP_LINE_HEIGHT, POPUP_HIGHLIGHT_COLOR, POPUP_FONT_SIZE));
    popup_box(body)
}
