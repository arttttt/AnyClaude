//! Popup widgets: a vertical selectable list ([`popup_list`]) and a
//! visible-range helper for long lists ([`fixed_row_window`]). Domain-agnostic —
//! the caller supplies pre-coloured rows + the selected index; this kit only
//! lays them out. Selection is passed IN (derived from app state each frame,
//! R12), never stored here.

use std::ops::Range;

use term_ui::{CrossAxis, Modifier, Modify, Sizing, Stack, Text};

use crate::Segment;

/// Build a vertical list where exactly one row (`selected`) is highlighted by a
/// full-width `hl_bg` background bar. Every row is `Sizing::Fixed(row_h)`; rows
/// are pre-coloured by the caller (the selected row brightened, the rest
/// dimmed). The stack is `CrossAxis::Stretch`, so the highlight `Block` — which
/// stretches its single child to its inner box — spans the full list width,
/// reproducing the immediate-mode highlight bar declaratively.
///
/// `selected` out of range highlights nothing (no panic); an empty `rows`
/// renders as an empty stack.
pub fn popup_list(
    rows: &[Segment],
    selected: usize,
    row_h: f32,
    hl_bg: [f32; 4],
    font_size: f32,
) -> Stack {
    let mut list = Stack::vstack().cross(CrossAxis::Stretch);
    for (i, row) in rows.iter().enumerate() {
        let text = Text::new(row.text.clone(), font_size, row.color);
        if i == selected {
            list = list.child_sized(
                text.modify(Modifier::new().background(hl_bg)),
                Sizing::Fixed(row_h),
            );
        } else {
            list = list.child_sized(text, Sizing::Fixed(row_h));
        }
    }
    list
}

/// The visible row range for a fixed-row scroll window (R11 virtualization):
/// given the current `scroll_offset`, the `total` row count, and how many rows
/// fit on screen (`max_visible`), return the `[start, end)` slice to render.
/// `start` is clamped so the window never scrolls past the last full page;
/// `end` never exceeds `total`. For `total <= max_visible` the whole list fits
/// and this is `0..total`.
pub fn fixed_row_window(scroll_offset: usize, total: usize, max_visible: usize) -> Range<usize> {
    let start = scroll_offset.min(total.saturating_sub(max_visible));
    start..(start + max_visible).min(total)
}
