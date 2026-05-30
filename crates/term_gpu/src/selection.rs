//! Linear (row-wrapping) text selection for a single VT grid.
//!
//! Modelled on Warp's `app/src/terminal/model/selection.rs`:
//! "A selection should start when the mouse is clicked, finalized
//!  when the button is released, cleared when text is added/removed
//!  /scrolled on the screen, and cleared if the user clicks off."
//!
//! Coordinates are absolute-row form: `row` indexes into
//! `RenderSnapshot::rows` (scrollback first, then visible) so the
//! selection stays anchored to its content as the viewport scrolls.
//! `col` is in cells `[0, cols)`.

use term_core::{CellFlags, RenderSnapshot};

use crate::{panel_render::PanelRect, CellMetrics, RectInstance};

/// Warp's `crates/warpui_core/src/text/words.rs::DEFAULT_WORD_BOUNDARY_CHARS`,
/// verbatim so double-click expansion matches Warp's behavior.
pub const WORD_BOUNDARY_CHARS: [char; 33] = [
    '`', '~', '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '-', '=', '+', '[', '{', ']', '}',
    '\\', '|', ';', ':', '\'', '"', ',', '.', '<', '>', '/', '?', '«', '»',
];

/// Tint for selected cells. Matches Warp's `text_selection_color`.
pub const SELECTION_COLOR: [f32; 4] = [118.0 / 255.0, 167.0 / 255.0, 250.0 / 255.0, 0.4];

pub fn is_word_boundary(c: char) -> bool {
    c.is_whitespace() || WORD_BOUNDARY_CHARS.contains(&c)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPoint {
    pub row: usize,
    pub col: usize,
}

impl PartialOrd for CellPoint {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CellPoint {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.row, self.col).cmp(&(other.row, other.col))
    }
}

/// Linear text selection inside a single grid.
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    /// Where the mouse first pressed down.
    pub anchor: CellPoint,
    /// Where the mouse currently is (or was released).
    pub cursor: CellPoint,
}

impl Selection {
    pub fn new(point: CellPoint) -> Self {
        Self {
            anchor: point,
            cursor: point,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Returns the normalized `(start, end)` range with `start <= end`
    /// in document order.
    pub fn range(&self) -> (CellPoint, CellPoint) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }
}

/// Expand a click point to the surrounding "word" by walking left and
/// right until hitting a word-boundary character of the opposite class
/// (or the row's edge). Modelled on Warp's `semantic_search_*` helpers
/// (`app/src/terminal/model/selection.rs:507-509`). Returns
/// `(start, end)` with `end.col` one past the last selected cell so it
/// matches the half-open range convention used in `push_selection_rects`.
pub fn expand_word(point: CellPoint, snapshot: &RenderSnapshot) -> (CellPoint, CellPoint) {
    let Some(row) = snapshot.rows.get(point.row) else {
        return (point, point);
    };
    let cells = &row.cells;
    if cells.is_empty() || point.col >= cells.len() {
        return (point, point);
    }
    let center_is_boundary = is_word_boundary(cells[point.col].c);
    let mut start_col = point.col;
    while start_col > 0 && is_word_boundary(cells[start_col - 1].c) == center_is_boundary {
        start_col -= 1;
    }
    let mut end_col = point.col;
    while end_col + 1 < cells.len()
        && is_word_boundary(cells[end_col + 1].c) == center_is_boundary
    {
        end_col += 1;
    }
    (
        CellPoint {
            row: point.row,
            col: start_col,
        },
        CellPoint {
            row: point.row,
            col: end_col + 1,
        },
    )
}

/// Expand a click point to its entire physical row.
pub fn expand_line(point: CellPoint, snapshot: &RenderSnapshot) -> (CellPoint, CellPoint) {
    let cols = snapshot
        .rows
        .get(point.row)
        .map(|r| r.cells.len())
        .unwrap_or(0);
    (
        CellPoint {
            row: point.row,
            col: 0,
        },
        CellPoint {
            row: point.row,
            col: cols,
        },
    )
}

/// Render the selection as plain text. Mirrors Warp's `bounds_to_string`
/// / `line_to_string` (`app/src/terminal/model/grid/grid_handler.rs`):
/// per-row trim of trailing blank cells, no newline between soft-wrapped
/// rows (continuation rows belong to the same logical line), newline
/// between hard-broken rows.
pub fn selection_to_text(sel: &Selection, snapshot: &RenderSnapshot) -> String {
    let (start, end) = sel.range();
    let mut out = String::new();
    for row_idx in start.row..=end.row {
        let Some(row) = snapshot.rows.get(row_idx) else { break };
        let col_start = if row_idx == start.row {
            start.col
        } else {
            0
        };
        let col_end = if row_idx == end.row {
            end.col.min(row.cells.len())
        } else {
            row.cells.len()
        };
        if col_start >= col_end {
            // Empty slice — still emit a newline between rows so a
            // multi-row selection over a blank line doesn't collapse.
        } else {
            // Trim trailing blank cells (` ` with default attributes)
            // so partial-row copies don't pad with spaces.
            let mut effective_end = col_end;
            while effective_end > col_start
                && row.cells[effective_end - 1].c == ' '
                && row.cells[effective_end - 1].flags.bits() == 0
            {
                effective_end -= 1;
            }
            for cell in &row.cells[col_start..effective_end] {
                out.push(cell.c);
            }
        }
        if row_idx < end.row {
            // Soft-wrap continuation → no newline. Hard-break → '\n'.
            let last = row.cells.last();
            let is_wrap = last
                .map(|c| c.flags.contains(CellFlags::WRAPLINE))
                .unwrap_or(false);
            if !is_wrap {
                out.push('\n');
            }
        }
    }
    out
}

/// Push a `RectInstance` for every cell inside the selection range,
/// using the same row positioning math as `populate_panel`. Selection
/// rects render after background rects (so they tint the bg) but before
/// glyphs (so text stays legible through the highlight).
///
/// Wide-span selections produce one rect per row span (already coalesced
/// per row) — at our cell counts (panel ≤ ~250×80 ≈ 20k cells max,
/// typical selection well under 1k cells) further coalescing would be
/// below the noise floor.
pub fn push_selection_rects(
    sel: &Selection,
    snapshot: &RenderSnapshot,
    panel_rect: PanelRect,
    scale_factor: f32,
    metrics: CellMetrics,
    scroll_offset_y_logical: f32,
    rects: &mut Vec<RectInstance>,
) {
    let (start, end) = sel.range();
    if start == end {
        return;
    }
    let sf = scale_factor;
    let cell_h_logical = metrics.height_physical / sf;
    let panel_origin_x_physical = panel_rect.x * sf;
    let panel_origin_y_physical = panel_rect.y * sf;
    let scroll_offset_y_physical = scroll_offset_y_logical * sf;
    let total_rows = snapshot.rows.len();
    let visible_rows = snapshot.visible_rows;
    let baseline_offset_phys =
        total_rows.saturating_sub(visible_rows) as f32 * metrics.height_physical;
    let panel_max_x_phys = panel_rect.w * sf;
    let panel_max_y_phys = panel_rect.h * sf;
    let end_row = end.row.min(total_rows.saturating_sub(1));
    for row_idx in start.row..=end_row {
        let Some(row) = snapshot.rows.get(row_idx) else {
            continue;
        };
        let row_y_phys = row_idx as f32 * metrics.height_physical - baseline_offset_phys
            + scroll_offset_y_physical;
        if row_y_phys + metrics.height_physical <= 0.0 || row_y_phys >= panel_max_y_phys {
            continue;
        }
        let cols = row.cells.len();
        // Linear (row-wrapping) selection: full row on intermediate
        // lines; clipped on first / last lines by start.col / end.col.
        let col_start = if row_idx == start.row { start.col } else { 0 };
        let col_end = if row_idx == end.row { end.col } else { cols };
        if col_start >= col_end {
            continue;
        }
        let span_cells = col_end - col_start;
        let span_w_phys = (span_cells as f32 * metrics.width_physical)
            .min(panel_max_x_phys - col_start as f32 * metrics.width_physical);
        if span_w_phys <= 0.0 {
            continue;
        }
        let pos_x_logical =
            (panel_origin_x_physical + col_start as f32 * metrics.width_physical) / sf;
        let pos_y_logical = (panel_origin_y_physical + row_y_phys) / sf;
        rects.push(RectInstance {
            pos: [pos_x_logical, pos_y_logical],
            size: [span_w_phys / sf, cell_h_logical],
            color: SELECTION_COLOR,
        });
    }
}
