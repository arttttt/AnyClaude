//! Reflow tests — column resize preserves content via WRAPLINE chains.
//!
//! Modelled on Warp's reflow (`warp_terminal::model::grid::flat_storage::Index::rebuild`),
//! adapted for our cell-based grid: instead of grapheme runs over a flat
//! byte buffer, we walk `Cell`s through WRAPLINE-joined rows.

use term_core::{CellFlags, TerminalEmulator, VtEmulator};

fn row_text(em: &VtEmulator, row: usize) -> String {
    em.snapshot().rows[row].cells.iter().map(|c| c.c).collect()
}

fn row_wraps(em: &VtEmulator, row: usize) -> bool {
    let snap = em.snapshot();
    let cols = snap.rows[row].cells.len();
    cols > 0 && snap.rows[row].cells[cols - 1].flags.wrap_line()
}

#[test]
fn shrink_wraps_overflow() {
    // "helloworld" in 10 cols fits on one row. Shrink to 5 cols → must
    // split into two rows joined by a WRAPLINE marker.
    let mut em = VtEmulator::new(10, 3, 0);
    em.process(b"helloworld");
    em.resize(5, 3);
    assert_eq!(&row_text(&em, 0)[..5], "hello");
    assert_eq!(&row_text(&em, 1)[..5], "world");
    assert!(row_wraps(&em, 0), "row 0 must be marked as soft-wrapped");
    assert!(!row_wraps(&em, 1), "row 1 ends the logical line");
}

#[test]
fn shrink_wraps_multiple_segments() {
    // "abcdefghij" (10 chars) in 3-col grid → 4 rows.
    let mut em = VtEmulator::new(10, 5, 0);
    em.process(b"abcdefghij");
    em.resize(3, 5);
    assert_eq!(&row_text(&em, 0)[..3], "abc");
    assert_eq!(&row_text(&em, 1)[..3], "def");
    assert_eq!(&row_text(&em, 2)[..3], "ghi");
    assert_eq!(&row_text(&em, 3)[..3], "j  ");
    assert!(row_wraps(&em, 0));
    assert!(row_wraps(&em, 1));
    assert!(row_wraps(&em, 2));
    assert!(!row_wraps(&em, 3));
}

#[test]
fn grow_unwraps_continuation() {
    // First produce a wrapped pair, then grow back — the two rows must
    // collapse to a single contiguous row.
    let mut em = VtEmulator::new(5, 3, 0);
    em.process(b"helloworld");
    // sanity: starts wrapped
    assert!(row_wraps(&em, 0));
    em.resize(10, 3);
    assert_eq!(&row_text(&em, 0)[..10], "helloworld");
    assert!(!row_wraps(&em, 0), "merged row must not be marked WRAPLINE");
}

#[test]
fn grow_preserves_hard_break() {
    // Two rows separated by CRLF — even a much wider grid must keep
    // them on separate rows.
    let mut em = VtEmulator::new(10, 3, 0);
    em.process(b"hello\r\nworld");
    em.resize(40, 3);
    assert_eq!(&row_text(&em, 0)[..5], "hello");
    assert_eq!(&row_text(&em, 1)[..5], "world");
    assert!(!row_wraps(&em, 0));
    assert!(!row_wraps(&em, 1));
}

#[test]
fn cursor_follows_content_on_shrink() {
    // After printing "hello" in a 10-col grid the cursor sits at col 5.
    // Shrinking to 3 cols should put the cursor at (row=1, col=2) —
    // logical offset 5 / 3 = row 1, offset 5 % 3 = col 2.
    let mut em = VtEmulator::new(10, 3, 0);
    em.process(b"hello");
    em.resize(3, 3);
    let snap = em.snapshot();
    assert_eq!(snap.cursor.row, 1, "row mismatch: snap={:?}", snap.cursor);
    assert_eq!(snap.cursor.col, 2, "col mismatch: snap={:?}", snap.cursor);
}

#[test]
fn cursor_follows_content_on_grow() {
    // Print "abcdef" in 3 cols (cursor at row 1, col 3 — phantom).
    // Capping pulls the offset to last printed cell (offset = 6).
    // Grow to 10 cols → offset 6 maps to (row 0, col 6).
    let mut em = VtEmulator::new(3, 4, 0);
    em.process(b"abcdef");
    em.resize(10, 4);
    let snap = em.snapshot();
    assert_eq!(snap.cursor.row, 0);
    assert_eq!(snap.cursor.col, 6);
}

#[test]
fn roundtrip_preserves_content() {
    // shrink-then-grow back to the original column count must yield
    // the same visible characters as before the resize.
    let mut em = VtEmulator::new(10, 3, 0);
    em.process(b"helloworld");
    let original: String = (0..3).map(|r| row_text(&em, r)).collect();
    em.resize(5, 3);
    em.resize(10, 3);
    let after: String = (0..3).map(|r| row_text(&em, r)).collect();
    assert_eq!(original, after);
}

#[test]
fn wrapline_repositions_to_new_boundary_after_grow() {
    // After shrinking to 3 cols then growing to 7 cols, the single
    // logical line "abcdefghij" should occupy two rows: "abcdefg"
    // (wrapped) and "hij" (terminal).
    let mut em = VtEmulator::new(10, 5, 0);
    em.process(b"abcdefghij");
    em.resize(3, 5);
    em.resize(7, 5);
    assert_eq!(&row_text(&em, 0)[..7], "abcdefg");
    assert_eq!(&row_text(&em, 1)[..3], "hij");
    assert!(row_wraps(&em, 0));
    assert!(!row_wraps(&em, 1));
}

#[test]
fn empty_grid_resize_is_noop() {
    // A grid with no printed content can shrink and grow freely without
    // panicking and without setting WRAPLINE anywhere.
    let mut em = VtEmulator::new(80, 24, 0);
    em.resize(40, 12);
    em.resize(100, 30);
    for r in 0..em.snapshot().rows.len() {
        assert!(!row_wraps(&em, r), "row {r} should not be marked WRAPLINE");
    }
}

#[test]
fn multiple_logical_lines_reflow_independently() {
    // Two hard-broken lines, each long enough to wrap on shrink.
    let mut em = VtEmulator::new(10, 5, 0);
    em.process(b"abcdef\r\nghijkl");
    em.resize(3, 5);
    // Line 1: "abcdef" → rows 0..=1 ("abc", "def")
    // Line 2: "ghijkl" → rows 2..=3 ("ghi", "jkl")
    assert_eq!(&row_text(&em, 0)[..3], "abc");
    assert_eq!(&row_text(&em, 1)[..3], "def");
    assert_eq!(&row_text(&em, 2)[..3], "ghi");
    assert_eq!(&row_text(&em, 3)[..3], "jkl");
    assert!(row_wraps(&em, 0));
    assert!(!row_wraps(&em, 1), "hard break between the two lines");
    assert!(row_wraps(&em, 2));
    assert!(!row_wraps(&em, 3));
}

#[test]
fn scrollback_participates_in_reflow() {
    // Push many lines so some go into scrollback, then shrink. Reflow
    // must walk those scrollback rows too — any logical line that
    // crosses the scrollback boundary must rewrap as a single piece.
    let mut em = VtEmulator::new(10, 4, 100);
    em.process(b"helloworld\r\n");
    // Add filler so the first row gets pushed into scrollback.
    for _ in 0..5 {
        em.process(b"...\r\n");
    }
    em.resize(5, 4);
    // The shrunk grid contains 4 visible rows; the wrapped "helloworld"
    // and 5 "..." rows now live in scrollback. We can't directly inspect
    // scrollback through the snapshot API, but we can roundtrip: grow
    // back to 10 and verify "helloworld" is still one continuous line.
    em.resize(10, 4);
    em.resize(80, 24);
    let snap = em.snapshot();
    let first = snap
        .rows
        .iter()
        .map(|r| r.cells.iter().map(|c| c.c).collect::<String>())
        .find(|s| s.starts_with("helloworld"));
    assert!(
        first.is_some(),
        "helloworld must survive scrollback round-trip; rows: {:?}",
        snap.rows
            .iter()
            .map(|r| r.cells.iter().map(|c| c.c).collect::<String>())
            .collect::<Vec<_>>()
    );
}

#[test]
fn shrink_clears_old_wrapline_flags() {
    // Earlier wrap boundaries from the old column count should not
    // leak into the new layout. Pre-shrink, the wrap is at col 9.
    // Post-shrink to 5 cols, the wrap should be at col 4 only.
    let mut em = VtEmulator::new(10, 3, 0);
    em.process(b"abcdefghijklmno"); // wraps once at col 9 ("abcdefghij"|"klmno")
    em.resize(5, 4);
    // Expected layout: "abcde" (w), "fghij" (w), "klmno" (no wrap)
    assert!(row_wraps(&em, 0));
    assert!(row_wraps(&em, 1));
    assert!(!row_wraps(&em, 2));
    // Verify the cell at the OLD wrap position (col 4 of row 1) does
    // NOT have stale WRAPLINE inherited from a prior reflow run.
    let snap = em.snapshot();
    let cell = &snap.rows[2].cells[4];
    assert!(
        !cell.flags.contains(CellFlags::WRAPLINE),
        "stale WRAPLINE leaked into row 2: cell={cell:?}"
    );
}
