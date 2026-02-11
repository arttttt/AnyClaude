//! Tests for startup readiness detection (cursor_hidden + cursor.row > 0)
//! and buffered input flush on Ready transition.

mod common;

use common::*;

// -- negative: conditions not met -> stays Attached ----------------------------

#[test]
fn stays_attached_cursor_visible_row_zero() {
    let (mut app, _, _emu) = make_app_with_pty();
    // Default: cursor visible at (0,0).
    app.on_pty_output();
    assert!(!app.is_pty_ready());
}

#[test]
fn stays_attached_cursor_hidden_row_zero() {
    let (mut app, _, emu) = make_app_with_pty();
    emu.lock().process(b"\x1b[?25l");
    app.on_pty_output();
    assert!(!app.is_pty_ready());
}

#[test]
fn stays_attached_cursor_visible_row_nonzero() {
    let (mut app, _, emu) = make_app_with_pty();
    emu.lock().process(b"\n");
    app.on_pty_output();
    assert!(!app.is_pty_ready());
}

// -- positive: both conditions met -> Ready ------------------------------------

#[test]
fn transitions_to_ready_cursor_hidden_and_row_nonzero() {
    let (mut app, _, emu) = make_app_with_pty();
    emu.lock().process(b"\x1b[?25l\n");
    app.on_pty_output();
    assert!(app.is_pty_ready());
}

// -- buffer flush on transition -----------------------------------------------

#[test]
fn flushes_buffered_input_on_ready() {
    let (mut app, spy_buf, emu) = make_app_with_pty();
    app.send_input(b"h");
    app.send_input(b"i");
    assert!(!app.is_pty_ready());
    assert!(spy_buf.lock().is_empty());

    emu.lock().process(b"\x1b[?25l\n");
    app.on_pty_output();
    assert!(app.is_pty_ready());

    let written = spy_buf.lock().clone();
    assert_eq!(written, b"hi");
}

#[test]
fn flushes_multiple_buffered_entries() {
    let (mut app, spy_buf, emu) = make_app_with_pty();
    app.send_input(b"a");
    app.on_paste("bc");
    app.send_input(b"d");

    emu.lock().process(b"\x1b[?25l\n");
    app.on_pty_output();
    assert!(app.is_pty_ready());

    let written = spy_buf.lock().clone();
    let mut expected = Vec::new();
    expected.extend_from_slice(b"a");
    expected.extend_from_slice(b"\x1b[200~bc\x1b[201~");
    expected.extend_from_slice(b"d");
    assert_eq!(written, expected);
}
