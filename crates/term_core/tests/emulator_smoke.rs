//! Emulator integration tests — feed bytes, snapshot, check grid state.

use term_core::{
    AnsiPalette, Cell, CursorStyle, PromptMarker, TermColor, TerminalEmulator, VtEmulator,
};

fn cells_text(row_cells: &[Cell]) -> String {
    row_cells.iter().map(|c| c.c).collect()
}

#[test]
fn prints_text_into_grid() {
    let mut em = VtEmulator::new(20, 4, 0);
    em.process(b"hello");
    let snap = em.snapshot();
    assert_eq!(&cells_text(&snap.rows[0].cells)[..5], "hello");
    assert_eq!(snap.cursor.row, 0);
    assert_eq!(snap.cursor.col, 5);
}

#[test]
fn linefeed_and_carriage_return() {
    let mut em = VtEmulator::new(10, 3, 0);
    em.process(b"abc\r\nxyz");
    let snap = em.snapshot();
    assert_eq!(&cells_text(&snap.rows[0].cells)[..3], "abc");
    assert_eq!(&cells_text(&snap.rows[1].cells)[..3], "xyz");
    assert_eq!(snap.cursor.row, 1);
    assert_eq!(snap.cursor.col, 3);
}

#[test]
fn cup_positions_cursor() {
    let mut em = VtEmulator::new(20, 5, 0);
    em.process(b"\x1b[3;7H");
    let snap = em.snapshot();
    // 1-based: row 3 col 7 -> 0-based (2, 6)
    assert_eq!(snap.cursor.row, 2);
    assert_eq!(snap.cursor.col, 6);
}

#[test]
fn sgr_colours_propagate_to_cells() {
    let mut em = VtEmulator::new(10, 2, 0);
    em.process(b"\x1b[1;31mhi\x1b[m");
    let snap = em.snapshot();
    let h = &snap.rows[0].cells[0];
    assert_eq!(h.c, 'h');
    assert!(h.flags.bold());
    assert_eq!(h.fg, TermColor::Indexed(1));
    // After reset, current attrs cleared — subsequent prints would be default.
}

#[test]
fn ech_clears_in_place_without_moving_cursor() {
    let mut em = VtEmulator::new(10, 1, 0);
    em.process(b"hello");
    // Move back 5, ECH 3.
    em.process(b"\x1b[5D\x1b[3X");
    let snap = em.snapshot();
    assert_eq!(snap.cursor.col, 0, "ECH must not move the cursor");
    assert_eq!(&cells_text(&snap.rows[0].cells)[..5], "   lo");
}

#[test]
fn ich_inserts_blank_cells() {
    let mut em = VtEmulator::new(10, 1, 0);
    em.process(b"hello\x1b[5D\x1b[2@");
    let snap = em.snapshot();
    assert_eq!(&cells_text(&snap.rows[0].cells)[..7], "  hello");
}

#[test]
fn dch_deletes_cells() {
    let mut em = VtEmulator::new(10, 1, 0);
    em.process(b"hello\x1b[5D\x1b[2P");
    let snap = em.snapshot();
    assert_eq!(&cells_text(&snap.rows[0].cells)[..3], "llo");
}

#[test]
fn rep_repeats_last_character() {
    let mut em = VtEmulator::new(10, 1, 0);
    em.process(b"-\x1b[4b");
    let snap = em.snapshot();
    assert_eq!(&cells_text(&snap.rows[0].cells)[..5], "-----");
}

#[test]
fn da_emits_vt102_reply() {
    let mut em = VtEmulator::new(10, 1, 0);
    em.process(b"\x1b[c");
    assert_eq!(em.take_responses(), b"\x1b[?6c".to_vec());
}

#[test]
fn dsr_cursor_position_reply() {
    let mut em = VtEmulator::new(20, 5, 0);
    em.process(b"\x1b[3;7H\x1b[6n");
    let reply = em.take_responses();
    assert_eq!(reply, b"\x1b[3;7R".to_vec());
}

#[test]
fn alt_screen_swap_preserves_main() {
    let mut em = VtEmulator::new(10, 2, 0);
    em.process(b"main");
    em.process(b"\x1b[?1049h"); // enter alt
    let alt = em.snapshot();
    assert_eq!(&cells_text(&alt.rows[0].cells)[..4], "    "); // cleared
    em.process(b"alt!");
    em.process(b"\x1b[?1049l"); // exit
    let back = em.snapshot();
    assert_eq!(&cells_text(&back.rows[0].cells)[..4], "main");
}

#[test]
fn osc_title_stored() {
    let mut em = VtEmulator::new(10, 1, 0);
    em.process(b"\x1b]0;Hello\x07");
    assert_eq!(em.title(), "Hello");
}

#[test]
fn osc_7_cwd_stored() {
    let mut em = VtEmulator::new(10, 1, 0);
    em.process(b"\x1b]7;file:///tmp\x07");
    let snap = em.snapshot();
    assert_eq!(snap.cwd.as_deref(), Some("file:///tmp"));
}

#[test]
fn osc_8_hyperlink_attaches_to_cells() {
    let mut em = VtEmulator::new(20, 1, 0);
    em.process(b"a\x1b]8;;https://x\x07hi\x1b]8;;\x07b");
    let snap = em.snapshot();
    // 'a' has no hyperlink, 'h' and 'i' do, 'b' doesn't.
    assert!(snap.rows[0].cells[0].hyperlink().is_none(), "'a' should not be linked");
    assert_eq!(snap.rows[0].cells[1].hyperlink(), Some("https://x"));
    assert_eq!(snap.rows[0].cells[2].hyperlink(), Some("https://x"));
    assert!(snap.rows[0].cells[3].hyperlink().is_none(), "'b' should not be linked");
}

#[test]
fn osc_133_prompt_marker_attached_to_next_cell_only() {
    let mut em = VtEmulator::new(20, 1, 0);
    em.process(b"\x1b]133;A\x07$ ");
    let snap = em.snapshot();
    // 'A' marker should attach to '$' (first printed cell), not ' '.
    let first = &snap.rows[0].cells[0];
    let space = &snap.rows[0].cells[1];
    assert_eq!(
        first.extra.as_ref().and_then(|e| e.prompt.clone()),
        Some(PromptMarker::Start)
    );
    assert!(space.extra.as_ref().and_then(|e| e.prompt.as_ref()).is_none());
}

#[test]
fn autowrap_carries_into_next_row() {
    let mut em = VtEmulator::new(4, 3, 0);
    em.process(b"abcdef"); // 4 cols → wrap after 4 chars
    let snap = em.snapshot();
    assert_eq!(&cells_text(&snap.rows[0].cells), "abcd");
    assert_eq!(&cells_text(&snap.rows[1].cells)[..2], "ef");
}

#[test]
fn autowrap_marks_wrapline_on_overflowed_row() {
    // The last cell of a row whose content continues on the next row
    // must carry CellFlags::WRAPLINE. The continuation row, when not
    // itself overflowed, must not.
    let mut em = VtEmulator::new(5, 3, 0);
    em.process(b"helloworld");
    let snap = em.snapshot();
    assert!(
        snap.rows[0].cells[4].flags.wrap_line(),
        "row 0 last cell must carry WRAPLINE after overflow"
    );
    assert!(
        !snap.rows[1].cells[4].flags.wrap_line(),
        "row 1 did not overflow further — WRAPLINE must stay clear"
    );
}

#[test]
fn hard_break_does_not_set_wrapline() {
    // CR/LF is a hard line break: the row being left must not be
    // marked WRAPLINE even if it ends at the right edge.
    let mut em = VtEmulator::new(5, 2, 0);
    em.process(b"hello\r\nworld");
    let snap = em.snapshot();
    assert!(
        !snap.rows[0].cells[4].flags.wrap_line(),
        "hard linebreak must not set WRAPLINE"
    );
}

#[test]
fn focus_reporting_emits_on_demand() {
    let mut em = VtEmulator::new(10, 1, 0);
    // Without DEC 1004, no output.
    em.emit_focus(true);
    assert!(em.take_responses().is_empty());
    // Enable DEC 1004, then focus event.
    em.process(b"\x1b[?1004h");
    em.emit_focus(true);
    assert_eq!(em.take_responses(), b"\x1b[I".to_vec());
    em.emit_focus(false);
    assert_eq!(em.take_responses(), b"\x1b[O".to_vec());
}

#[test]
fn cursor_style_set_via_decscusr() {
    let mut em = VtEmulator::new(10, 1, 0);
    em.process(b"\x1b[5 q"); // beam blink
    let snap = em.snapshot();
    assert_eq!(snap.cursor.style, CursorStyle::BeamBlink);
}

#[test]
fn palette_resolves_indexed_colours() {
    let palette = AnsiPalette::default_dark();
    let c = TermColor::Indexed(1);
    let rgba = c.to_rgba(&palette);
    // Red base from default_dark is non-zero.
    assert!(rgba[0] > 0.0, "red channel of palette[1] should be non-zero");
}

#[test]
fn resize_grows_grid_to_taller_visible_region() {
    // Regression: a previous Grid::resize built the target row count
    // from `visible_start() + rows`, which itself depended on the
    // still-old `visible_rows`. Each push grew `visible_start()` by 1
    // in lock-step, so the loop never terminated when `rows >
    // visible_rows`.
    let mut em = VtEmulator::new(80, 24, 0);
    em.process(b"hello");
    em.resize(120, 40);
    let snap = em.snapshot();
    assert_eq!(snap.rows.len(), 40, "visible row count should be 40");
    assert_eq!(snap.rows[0].cells.len(), 120, "row width should be 120");
    assert_eq!(snap.rows[0].cells[0].c, 'h', "original content preserved");
}

#[test]
fn resize_shrinks_visible_region_into_scrollback() {
    let mut em = VtEmulator::new(80, 24, 100);
    em.process(b"top\n");
    em.resize(80, 10);
    let snap = em.snapshot();
    assert_eq!(snap.rows.len(), 10);
}

#[test]
fn resize_keeps_top_content_anchored_through_shrink_and_grow() {
    // Top-anchored resize: the visible region pins to row 0 of the row
    // buffer. Top content never scrolls in response to window resizes
    // (matches the user's Warp configuration). A shrink truncates blank
    // rows at the bottom and a subsequent grow pads with blanks again,
    // both leaving the original text exactly where it was.
    let mut em = VtEmulator::new(80, 24, 100);
    em.process(b"hello world");
    em.resize(80, 10);
    em.resize(80, 24);
    let snap = em.snapshot();
    let first_row: String = snap.rows[0].cells.iter().map(|c| c.c).collect();
    assert!(
        first_row.starts_with("hello world"),
        "expected first visible row to start with 'hello world', got {first_row:?}"
    );
}
