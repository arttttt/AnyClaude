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

use term_ui::{Block, BlockShadow, BlockStyle, BoxView, CrossAxis, Insets, Sizing, Stack, Text};
use uikit::{fixed_row_window, popup_list, Segment};

use crate::config::SettingsFieldSnapshot;
use crate::ui::app_state::AppState;
use crate::ui::backend_switch::{BackendPopupSection, BackendSwitchState};
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
/// Green status suffix (`[Active]` / `[Selected]`) — mirrors `chrome::CHROME_FLASH_COLOR`.
const POPUP_STATUS_COLOR: [f32; 4] = [0.4, 0.85, 0.4, 1.0];

/// `cosmic_text::Weight::BOLD.0` — titles + section headers.
const WEIGHT_BOLD: u16 = 700;

/// The popup view for the AppState-only popups — history + settings — or `None`
/// when neither is open. The backend-switch popup needs runtime data AppState
/// doesn't carry (the backend list + active/override ids), so the coordinator
/// builds it via [`backend_view`] directly; the two feed the same second-tree
/// plumbing. Popups are mutually exclusive, so at most one is ever open.
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

/// One popup text run at the popup font size.
fn text(s: impl Into<String>, color: [f32; 4]) -> Text {
    Text::new(s, POPUP_FONT_SIZE, color)
}

/// The borderless, shadowless highlight Block behind a selected backend row.
fn row_highlight_style() -> BlockStyle {
    BlockStyle {
        background: POPUP_HIGHLIGHT_COLOR,
        border_color: [0.0; 4],
        border_width: 0.0,
        padding: Insets::default(),
        shadow: None,
    }
}

/// The numbered row prefix: `"  → {n}. "` when selected, `"    {n}. "` otherwise.
fn numbered_prefix(n: usize, selected: bool) -> String {
    if selected {
        format!("  → {n}. ")
    } else {
        format!("    {n}. ")
    }
}

/// One backend-section row: `prefix` + `name` (row colour) + an optional green
/// `[status]` suffix, assembled as an hstack of Text runs so the suffix can be a
/// different colour (which `popup_list`'s single-colour rows can't express).
/// When `selected`, the row is wrapped in a full-width highlight Block. Boxed so
/// selected (Block) and unselected (Stack) rows share one `child_boxed` slot.
fn backend_row(prefix: &str, name: &str, status: Option<&str>, selected: bool) -> BoxView {
    let row_color = if selected { POPUP_SELECTED_COLOR } else { POPUP_TEXT_COLOR };
    let mut row = Stack::hstack()
        .child(text(prefix.to_string(), row_color))
        .child(text(name.to_string(), row_color));
    if let Some(status_text) = status {
        row = row
            .child(text("  [", row_color))
            .child(text(status_text.to_string(), POPUP_STATUS_COLOR))
            .child(text("]", row_color));
    }
    if selected {
        Box::new(Block::new(row_highlight_style(), row))
    } else {
        Box::new(row)
    }
}

/// A backend section: a BOLD header (`"▸ "`/`"  "` + label, bright when this is
/// the active section), a dim separator rule, then the rows.
fn backend_section(label: &str, is_active: bool, rows: Vec<BoxView>) -> Stack {
    let header_prefix = if is_active { "▸ " } else { "  " };
    let header_color = if is_active { POPUP_SELECTED_COLOR } else { POPUP_TEXT_COLOR };
    let mut section = Stack::vstack()
        .cross(CrossAxis::Stretch)
        .child_sized(
            text(format!("{header_prefix}{label}"), header_color).weight(WEIGHT_BOLD),
            Sizing::Fixed(POPUP_LINE_HEIGHT),
        )
        .child_sized(
            text("  ──────────────────", POPUP_TEXT_COLOR),
            Sizing::Fixed(POPUP_LINE_HEIGHT),
        );
    for row in rows {
        section = section.child_boxed(row, Sizing::Fixed(POPUP_LINE_HEIGHT));
    }
    section
}

/// A Subagent / Teammate override section: a "Disabled (use active backend)"
/// leader at selection index 0 (with `[Active]` when no override is set), then
/// one row per backend (with `[Selected]` on the current override).
fn override_section(
    label: &str,
    items_and_ids: &[(String, String)],
    current_id: Option<&str>,
    selection: usize,
    is_active: bool,
) -> Stack {
    let disabled_selected = is_active && selection == 0;
    let disabled_prefix = if disabled_selected { "  → " } else { "    " };
    let disabled_status = current_id.is_none().then_some("Active");
    let mut rows: Vec<BoxView> = vec![backend_row(
        disabled_prefix,
        "Disabled (use active backend)",
        disabled_status,
        disabled_selected,
    )];
    for (idx, (name, id)) in items_and_ids.iter().enumerate() {
        let selected = is_active && selection == idx + 1;
        let status = (current_id == Some(id.as_str())).then_some("Selected");
        let prefix = numbered_prefix(idx + 1, selected);
        rows.push(backend_row(&prefix, name, status, selected));
    }
    backend_section(label, is_active, rows)
}

/// Backend switch popup: a bright title, then three sections — Active backend,
/// Subagent override, Teammate override — and a footer hint. Tab cycles the
/// active section (its header brightens with a `▸` arrow); only the active
/// section shows a selection highlight. Reproduces the immediate-mode layout
/// declaratively. Built by the coordinator (not `popup_view`) because it needs
/// the backend list + active/override ids that AppState doesn't carry.
pub fn backend_view(
    state: &BackendSwitchState,
    items_and_ids: &[(String, String)],
    active_backend: &str,
    current_subagent: Option<&str>,
    current_teammate: Option<&str>,
) -> Block {
    let (active_section, backend_sel, subagent_sel, teammate_sel) = match state {
        BackendSwitchState::Visible {
            section,
            backend_selection,
            subagent_selection,
            teammate_selection,
            ..
        } => (*section, *backend_selection, *subagent_selection, *teammate_selection),
        // Not reached (the coordinator only calls this when visible); benign box.
        BackendSwitchState::Hidden => {
            return popup_box(
                Stack::vstack()
                    .cross(CrossAxis::Stretch)
                    .child(title_row("Select Backend", POPUP_SELECTED_COLOR)),
            );
        }
    };

    let active_in = active_section == BackendPopupSection::ActiveBackend;
    let active_rows: Vec<BoxView> = items_and_ids
        .iter()
        .enumerate()
        .map(|(idx, (name, id))| {
            let selected = active_in && idx == backend_sel;
            let status = (id == active_backend).then_some("Active");
            backend_row(&numbered_prefix(idx + 1, selected), name, status, selected)
        })
        .collect();

    let body = Stack::vstack()
        .cross(CrossAxis::Stretch)
        .child_sized(
            title_row("Select Backend", POPUP_SELECTED_COLOR),
            Sizing::Fixed(POPUP_LINE_HEIGHT),
        )
        .spacer(Sizing::Fixed(POPUP_LINE_HEIGHT))
        .child(backend_section("Active Backend", active_in, active_rows))
        .spacer(Sizing::Fixed(POPUP_LINE_HEIGHT))
        .child(override_section(
            "Subagent Backend",
            items_and_ids,
            current_subagent,
            subagent_sel,
            active_section == BackendPopupSection::SubagentBackend,
        ))
        .spacer(Sizing::Fixed(POPUP_LINE_HEIGHT))
        .child(override_section(
            "Teammate Backend",
            items_and_ids,
            current_teammate,
            teammate_sel,
            active_section == BackendPopupSection::TeammateBackend,
        ))
        .spacer(Sizing::Fixed(POPUP_LINE_HEIGHT))
        .child_sized(
            text(
                "Tab: Section  ↑/↓: Move  Enter: Select  Del: Clear  Esc: Close",
                POPUP_TEXT_COLOR,
            ),
            Sizing::Fixed(POPUP_LINE_HEIGHT),
        );
    popup_box(body)
}
