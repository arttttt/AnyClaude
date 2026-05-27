//! Popup overlays for the GPU UI: backend switch, history, settings.
//!
//! Each popup is a free function that takes the popup's MVI state +
//! the renderer's mutable side channels (atlas / shape cache / glyphs
//! / rects / shadows) and pushes the appropriate instances. The
//! `RenderLayer` overlay then draws these on top of the terminal panel.
//!
//! Layout helpers:
//! - `draw_popup_chrome` — shadow + bg + 1px border for any popup rect.
//! - `draw_string_list_popup` — generic centred list (history, settings).
//! - `push_section_header` / `push_backend_item` /
//!   `push_override_section_rows` — backend popup section builders.
//!
//! Colour palette comes from [`super::chrome`] (the chrome and popup
//! UI share the dim-grey / status-green vocabulary).

use term_gpu::{
    measure_label_width, push_label, FontSystem, GlyphAtlas, GlyphInstance, RectInstance,
    ShadowInstance, Style, SwashCache, TextShapeCache, Weight,
};

use super::chrome::{CHROME_FLASH_COLOR, CHROME_TEXT_COLOR};
use crate::config::Backend;
use crate::ui::backend_switch::{BackendPopupSection, BackendSwitchState};
use crate::ui::history::HistoryDialogState;
use crate::ui::settings::SettingsDialogState;

/// Popup background color (dark grey with full alpha — opaque so the
/// content underneath is hidden).
const POPUP_BG_COLOR: [f32; 4] = [0.12, 0.12, 0.14, 1.0];
/// Popup item highlight (selected row).
const POPUP_HIGHLIGHT_COLOR: [f32; 4] = [0.22, 0.30, 0.42, 1.0];
/// Popup border / frame (subtle).
const POPUP_BORDER_COLOR: [f32; 4] = [0.30, 0.30, 0.35, 1.0];
/// Popup drop shadow color — soft black at 45% opacity.
const POPUP_SHADOW_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.45];
/// Drop shadow blur radius (logical px).
const POPUP_SHADOW_BLUR: f32 = 24.0;
/// Drop shadow downward offset (logical px).
const POPUP_SHADOW_OFFSET_Y: f32 = 8.0;
/// Rounded-corner radius for popup background.
const POPUP_CORNER_RADIUS: f32 = 6.0;
/// Popup line height (logical px) for items.
const POPUP_LINE_HEIGHT: f32 = 22.0;
/// Popup font size for items.
const POPUP_FONT_SIZE: f32 = 13.0;
/// Padding around popup content (logical px).
const POPUP_PADDING: f32 = 12.0;
/// Default popup width when items are short.
const POPUP_MIN_WIDTH: f32 = 280.0;

/// Brighter foreground for the selected popup item to contrast with
/// the highlight bar.
const DEFAULT_FG_FOR_POPUP_SELECTED: [f32; 4] = [0.95, 0.95, 0.95, 1.0];

/// Map an override-section selection index into the backend id it
/// represents. Index 0 is the "Disabled" leader (returns `None`);
/// indices 1..=N map to `backends[i - 1]`. Out-of-range indices fall
/// back to `None` so a stale state never panics.
pub(super) fn override_selection_to_backend_id(
    backends: &[Backend],
    selection: usize,
) -> Option<String> {
    if selection == 0 {
        return None;
    }
    backends.get(selection - 1).map(|b| b.name.clone())
}

/// Backend popup with three independent sections — Active, Subagent,
/// Teammate. Tab cycles the active section; Up/Down move selection
/// within the active section. Enter applies the section's action;
/// Del/Backspace resets the override sections back to Disabled.
///
/// Mirrors the legacy ratatui chrome (`backend_switch::dialog`) layout
/// 1:1 so users coming from the old UI find the same affordances.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_backend_switch_popup(
    state: &BackendSwitchState,
    items_and_ids: &[(String, String)],
    active_backend: &str,
    current_subagent: Option<&str>,
    current_teammate: Option<&str>,
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    shadows: &mut Vec<ShadowInstance>,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    window_w: f32,
    window_h: f32,
    sf: f32,
) {
    let (active_section, backend_sel, subagent_sel, teammate_sel) = match *state {
        BackendSwitchState::Visible {
            section,
            backend_selection,
            subagent_selection,
            teammate_selection,
            ..
        } => (
            section,
            backend_selection,
            subagent_selection,
            teammate_selection,
        ),
        BackendSwitchState::Hidden => return,
    };
    let n = items_and_ids.len();
    if n == 0 {
        return;
    }

    let title = "Select Backend";
    let footer_hint = "Tab: Section  ↑/↓: Move  Enter: Select  Del: Clear  Esc: Close";
    let separator = "  ──────────────────";

    let measure = |fs: &mut FontSystem,
                   sc: &mut TextShapeCache,
                   s: &str,
                   weight: Weight,
                   style: Style|
     -> f32 { measure_label_width(fs, sc, s, POPUP_FONT_SIZE, sf, weight, style) };

    // Width: longest of title / footer / headers / longest formatted item.
    let mut content_w = measure(
        font_system,
        ui_shape_cache,
        title,
        Weight::BOLD,
        Style::Normal,
    );
    content_w = content_w.max(measure(
        font_system,
        ui_shape_cache,
        footer_hint,
        Weight::NORMAL,
        Style::Normal,
    ));
    for header in &[
        "▸ Active Backend",
        "▸ Subagent Backend",
        "▸ Teammate Backend",
    ] {
        content_w = content_w.max(measure(
            font_system,
            ui_shape_cache,
            header,
            Weight::BOLD,
            Style::Normal,
        ));
    }
    // Longest possible item row: "  → 10. <name>  [Selected]".
    let max_name_w = items_and_ids
        .iter()
        .map(|(name, _)| {
            measure(
                font_system,
                ui_shape_cache,
                name,
                Weight::NORMAL,
                Style::Normal,
            )
        })
        .fold(0.0_f32, f32::max);
    let prefix_w = measure(
        font_system,
        ui_shape_cache,
        "  → 10. ",
        Weight::NORMAL,
        Style::Normal,
    );
    let suffix_w = measure(
        font_system,
        ui_shape_cache,
        "  [Selected]",
        Weight::NORMAL,
        Style::Normal,
    );
    content_w = content_w.max(prefix_w + max_name_w + suffix_w);
    let disabled_w = measure(
        font_system,
        ui_shape_cache,
        "    Disabled (use active backend)  [Active]",
        Weight::NORMAL,
        Style::Normal,
    );
    content_w = content_w.max(disabled_w);

    let width = (content_w + POPUP_PADDING * 2.0).max(POPUP_MIN_WIDTH);

    // Total rows: title + gap + (header + sep + items) per section + gaps + footer.
    let active_rows = n as f32;
    let override_rows = (n + 1) as f32;
    let total_rows: f32 = 1.0  // title
        + 1.0                  // gap before sections
        + 1.0 + 1.0 + active_rows   // Active section
        + 1.0                  // gap
        + 1.0 + 1.0 + override_rows // Subagent
        + 1.0                  // gap
        + 1.0 + 1.0 + override_rows // Teammate
        + 1.0                  // gap before footer
        + 1.0; // footer
    let height = POPUP_PADDING * 2.0 + total_rows * POPUP_LINE_HEIGHT;

    let x = ((window_w - width) * 0.5).max(0.0);
    let y = ((window_h - height) * 0.25).max(0.0);

    draw_popup_chrome(x, y, width, height, shadows, rects);

    let content_x = x + POPUP_PADDING;
    let mut cursor_y = y + POPUP_PADDING + POPUP_LINE_HEIGHT * 0.75;

    // Title.
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        title,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::BOLD,
        Style::Normal,
        DEFAULT_FG_FOR_POPUP_SELECTED,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    cursor_y += POPUP_LINE_HEIGHT;

    // --- Active Backend section ---
    push_section_header(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        "Active Backend",
        active_section == BackendPopupSection::ActiveBackend,
        content_x,
        cursor_y,
        sf,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        separator,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    for (idx, (name, id)) in items_and_ids.iter().enumerate() {
        let in_section = active_section == BackendPopupSection::ActiveBackend;
        let selected = in_section && idx == backend_sel;
        let status = if id == active_backend {
            Some(("Active", CHROME_FLASH_COLOR))
        } else {
            None
        };
        push_backend_item(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            rects,
            x,
            width,
            content_x,
            cursor_y,
            sf,
            idx + 1,
            name,
            status,
            selected,
        );
        cursor_y += POPUP_LINE_HEIGHT;
    }
    cursor_y += POPUP_LINE_HEIGHT;

    // --- Subagent Backend section ---
    push_section_header(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        "Subagent Backend",
        active_section == BackendPopupSection::SubagentBackend,
        content_x,
        cursor_y,
        sf,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        separator,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    cursor_y += push_override_section_rows(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        rects,
        items_and_ids,
        current_subagent,
        subagent_sel,
        active_section == BackendPopupSection::SubagentBackend,
        x,
        width,
        content_x,
        cursor_y,
        sf,
    );
    cursor_y += POPUP_LINE_HEIGHT;

    // --- Teammate Backend section ---
    push_section_header(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        "Teammate Backend",
        active_section == BackendPopupSection::TeammateBackend,
        content_x,
        cursor_y,
        sf,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        separator,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    cursor_y += push_override_section_rows(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        rects,
        items_and_ids,
        current_teammate,
        teammate_sel,
        active_section == BackendPopupSection::TeammateBackend,
        x,
        width,
        content_x,
        cursor_y,
        sf,
    );
    cursor_y += POPUP_LINE_HEIGHT;

    // --- Footer hint ---
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        footer_hint,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
}

/// Draw a section header line prefixed with `▸` when this section is
/// the active one (Tab target) and two spaces otherwise.
#[allow(clippy::too_many_arguments)]
fn push_section_header(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    ui_shape_cache: &mut TextShapeCache,
    glyphs: &mut Vec<GlyphInstance>,
    label: &str,
    is_active: bool,
    content_x: f32,
    cursor_y: f32,
    sf: f32,
) {
    let prefix = if is_active { "▸ " } else { "  " };
    let line = format!("{prefix}{label}");
    let color = if is_active {
        DEFAULT_FG_FOR_POPUP_SELECTED
    } else {
        CHROME_TEXT_COLOR
    };
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        &line,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::BOLD,
        Style::Normal,
        color,
    );
}

/// Draw a single backend item row: optional highlight bar, numbered
/// prefix, display name, optional `[status]` suffix in `status_color`.
#[allow(clippy::too_many_arguments)]
fn push_backend_item(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    ui_shape_cache: &mut TextShapeCache,
    glyphs: &mut Vec<GlyphInstance>,
    rects: &mut Vec<RectInstance>,
    popup_x: f32,
    popup_w: f32,
    content_x: f32,
    cursor_y: f32,
    sf: f32,
    item_index: usize,
    display_name: &str,
    status: Option<(&str, [f32; 4])>,
    is_selected: bool,
) {
    if is_selected {
        rects.push(RectInstance {
            pos: [popup_x + 1.0, cursor_y - POPUP_LINE_HEIGHT * 0.75],
            size: [popup_w - 2.0, POPUP_LINE_HEIGHT],
            color: POPUP_HIGHLIGHT_COLOR,
        });
    }
    let prefix = if is_selected {
        format!("  → {}. ", item_index)
    } else {
        format!("    {}. ", item_index)
    };
    let row_color = if is_selected {
        DEFAULT_FG_FOR_POPUP_SELECTED
    } else {
        CHROME_TEXT_COLOR
    };
    let mut x_cursor = push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        &prefix,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        row_color,
    );
    x_cursor = push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        display_name,
        x_cursor,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        row_color,
    );
    if let Some((status_text, color)) = status {
        x_cursor = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            "  [",
            x_cursor,
            cursor_y,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            row_color,
        );
        x_cursor = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            status_text,
            x_cursor,
            cursor_y,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            color,
        );
        let _ = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            "]",
            x_cursor,
            cursor_y,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            row_color,
        );
    }
}

/// Draw the body of a Subagent / Teammate section: a "Disabled" row
/// first (selection index 0), then each backend with a `[Selected]`
/// status when it matches the override's current value. Returns the
/// total vertical advance (rows × `POPUP_LINE_HEIGHT`) so the caller
/// can position whatever follows.
#[allow(clippy::too_many_arguments)]
fn push_override_section_rows(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    ui_shape_cache: &mut TextShapeCache,
    glyphs: &mut Vec<GlyphInstance>,
    rects: &mut Vec<RectInstance>,
    items_and_ids: &[(String, String)],
    current_id: Option<&str>,
    selection: usize,
    in_section: bool,
    popup_x: f32,
    popup_w: f32,
    content_x: f32,
    cursor_y: f32,
    sf: f32,
) -> f32 {
    let mut local_y = cursor_y;

    // Row 0 — Disabled.
    let disabled_selected = in_section && selection == 0;
    if disabled_selected {
        rects.push(RectInstance {
            pos: [popup_x + 1.0, local_y - POPUP_LINE_HEIGHT * 0.75],
            size: [popup_w - 2.0, POPUP_LINE_HEIGHT],
            color: POPUP_HIGHLIGHT_COLOR,
        });
    }
    let disabled_color = if disabled_selected {
        DEFAULT_FG_FOR_POPUP_SELECTED
    } else {
        CHROME_TEXT_COLOR
    };
    let prefix = if disabled_selected { "  → " } else { "    " };
    let mut x_cursor = push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        prefix,
        content_x,
        local_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        disabled_color,
    );
    x_cursor = push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        "Disabled (use active backend)",
        x_cursor,
        local_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        disabled_color,
    );
    if current_id.is_none() {
        let _ = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            "  [Active]",
            x_cursor,
            local_y,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            CHROME_FLASH_COLOR,
        );
    }
    local_y += POPUP_LINE_HEIGHT;

    // Rows 1..=n — backends, idx+1 in the override selection space.
    for (idx, (name, id)) in items_and_ids.iter().enumerate() {
        let selection_idx = idx + 1;
        let selected = in_section && selection == selection_idx;
        let status = if current_id == Some(id.as_str()) {
            Some(("Selected", CHROME_FLASH_COLOR))
        } else {
            None
        };
        push_backend_item(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            rects,
            popup_x,
            popup_w,
            content_x,
            local_y,
            sf,
            idx + 1,
            name,
            status,
            selected,
        );
        local_y += POPUP_LINE_HEIGHT;
    }

    local_y - cursor_y
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_history_popup(
    state: &HistoryDialogState,
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    shadows: &mut Vec<ShadowInstance>,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    window_w: f32,
    window_h: f32,
    sf: f32,
) {
    let (entries, scroll_offset) = match state {
        HistoryDialogState::Visible {
            entries,
            scroll_offset,
        } => (entries, *scroll_offset),
        HistoryDialogState::Hidden => return,
    };
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
    draw_string_list_popup(
        "History",
        &items,
        scroll_offset.min(items.len().saturating_sub(1)),
        Some("(no history yet)"),
        atlas,
        font_system,
        swash_cache,
        ui_shape_cache,
        shadows,
        rects,
        glyphs,
        window_w,
        window_h,
        sf,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_settings_popup(
    state: &SettingsDialogState,
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    shadows: &mut Vec<ShadowInstance>,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    window_w: f32,
    window_h: f32,
    sf: f32,
) {
    let (fields, focused) = match state {
        SettingsDialogState::Visible {
            fields, focused, ..
        } => (fields, *focused),
        SettingsDialogState::Hidden => return,
    };
    // Format each row as "[x] Label" / "[ ] Label" — checkbox is a
    // glyph prefix; the focus highlight comes from
    // draw_string_list_popup's selection bar.
    let formatted: Vec<String> = fields
        .iter()
        .map(|f| {
            let mark = if f.value { "[x]" } else { "[ ]" };
            format!("{mark}  {}", f.label)
        })
        .collect();
    draw_string_list_popup(
        "Settings  ·  Space toggle · Enter save · Esc cancel",
        &formatted,
        focused,
        None,
        atlas,
        font_system,
        swash_cache,
        ui_shape_cache,
        shadows,
        rects,
        glyphs,
        window_w,
        window_h,
        sf,
    );
}

/// Render a centered popup whose body is a list of strings with one
/// row visually selected. Used by both the backend-switch popup
/// (actionable selection) and the history popup (read-only browse).
/// When `items` is empty and an `empty_placeholder` is supplied, the
/// placeholder renders in dim grey instead of an item list.
#[allow(clippy::too_many_arguments)]
fn draw_string_list_popup(
    title: &str,
    items: &[String],
    selected: usize,
    empty_placeholder: Option<&str>,
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    shadows: &mut Vec<ShadowInstance>,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    window_w: f32,
    window_h: f32,
    sf: f32,
) {
    // Width: max of title / item / placeholder widths plus padding,
    // clamped to a sane minimum so a short list doesn't render as a
    // thin sliver.
    let title_w = measure_label_width(
        font_system,
        ui_shape_cache,
        title,
        POPUP_FONT_SIZE,
        sf,
        Weight::BOLD,
        Style::Normal,
    );
    let mut content_w = title_w;
    for item in items {
        let w = measure_label_width(
            font_system,
            ui_shape_cache,
            item,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
        );
        if w > content_w {
            content_w = w;
        }
    }
    if items.is_empty() {
        if let Some(p) = empty_placeholder {
            let w = measure_label_width(
                font_system,
                ui_shape_cache,
                p,
                POPUP_FONT_SIZE,
                sf,
                Weight::NORMAL,
                Style::Italic,
            );
            if w > content_w {
                content_w = w;
            }
        }
    }
    let width = (content_w + POPUP_PADDING * 2.0).max(POPUP_MIN_WIDTH);
    let body_rows = if items.is_empty() { 1.0 } else { items.len() as f32 };
    let height = POPUP_PADDING * 2.0
        + POPUP_LINE_HEIGHT          // title row
        + POPUP_LINE_HEIGHT * 0.5    // gap
        + body_rows * POPUP_LINE_HEIGHT;

    let x = ((window_w - width) * 0.5).max(0.0);
    let y = ((window_h - height) * 0.4).max(0.0);

    draw_popup_chrome(x, y, width, height, shadows, rects);

    let content_x = x + POPUP_PADDING;
    let mut cursor_y = y + POPUP_PADDING + POPUP_LINE_HEIGHT * 0.75;
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        title,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::BOLD,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    cursor_y += POPUP_LINE_HEIGHT * 0.5;
    let item_top_y = cursor_y - POPUP_LINE_HEIGHT * 0.75;

    if items.is_empty() {
        if let Some(p) = empty_placeholder {
            push_label(
                font_system,
                swash_cache,
                atlas,
                ui_shape_cache,
                glyphs,
                p,
                content_x,
                cursor_y,
                POPUP_FONT_SIZE,
                sf,
                Weight::NORMAL,
                Style::Italic,
                CHROME_TEXT_COLOR,
            );
        }
        return;
    }

    let highlight_y = item_top_y + (selected as f32) * POPUP_LINE_HEIGHT;
    rects.push(RectInstance {
        pos: [x + 1.0, highlight_y],
        size: [width - 2.0, POPUP_LINE_HEIGHT],
        color: POPUP_HIGHLIGHT_COLOR,
    });

    for (idx, item) in items.iter().enumerate() {
        let color = if idx == selected {
            DEFAULT_FG_FOR_POPUP_SELECTED
        } else {
            CHROME_TEXT_COLOR
        };
        push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            item,
            content_x,
            cursor_y,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            color,
        );
        cursor_y += POPUP_LINE_HEIGHT;
    }
}

/// Push the shadow + background + 1px-border frame for any popup
/// rectangle.
fn draw_popup_chrome(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    shadows: &mut Vec<ShadowInstance>,
    rects: &mut Vec<RectInstance>,
) {
    shadows.push(ShadowInstance {
        pos: [x, y],
        size: [width, height],
        blur_radius: POPUP_SHADOW_BLUR,
        corner_radius: POPUP_CORNER_RADIUS,
        offset: [0.0, POPUP_SHADOW_OFFSET_Y],
        color: POPUP_SHADOW_COLOR,
    });
    rects.push(RectInstance {
        pos: [x, y],
        size: [width, height],
        color: POPUP_BG_COLOR,
    });
    rects.push(RectInstance {
        pos: [x, y],
        size: [width, 1.0],
        color: POPUP_BORDER_COLOR,
    });
    rects.push(RectInstance {
        pos: [x, y + height - 1.0],
        size: [width, 1.0],
        color: POPUP_BORDER_COLOR,
    });
    rects.push(RectInstance {
        pos: [x, y],
        size: [1.0, height],
        color: POPUP_BORDER_COLOR,
    });
    rects.push(RectInstance {
        pos: [x + width - 1.0, y],
        size: [1.0, height],
        color: POPUP_BORDER_COLOR,
    });
}
