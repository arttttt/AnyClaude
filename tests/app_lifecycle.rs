//! Tests for App PTY lifecycle state machine and input buffering.

mod common;

use anyclaude::ui::pty::{PtyIntent, PtyLifecycleState};
use common::*;
use crossterm::event::KeyCode;

// -- is_pty_ready lifecycle ---------------------------------------------------

#[test]
fn not_ready_in_pending_state() {
    let app = make_app();
    assert!(!app.is_pty_ready());
}

#[test]
fn not_ready_in_attached_state() {
    let mut app = make_app();
    app.dispatch_pty(PtyIntent::Attach);
    assert!(!app.is_pty_ready());
}

#[test]
fn ready_after_reducer_got_output() {
    let mut app = make_app();
    app.dispatch_pty(PtyIntent::Attach);
    app.dispatch_pty(PtyIntent::GotOutput);
    assert!(app.is_pty_ready());
}

// -- on_pty_output without pty_handle -----------------------------------------

#[test]
fn on_pty_output_without_pty_handle_stays_attached() {
    let mut app = make_app();
    app.dispatch_pty(PtyIntent::Attach);
    app.on_pty_output();
    assert!(!app.is_pty_ready());
}

#[test]
fn on_pty_output_noop_when_already_ready() {
    let mut app = make_app();
    app.dispatch_pty(PtyIntent::Attach);
    app.dispatch_pty(PtyIntent::GotOutput);
    assert!(app.is_pty_ready());
    app.on_pty_output();
    assert!(app.is_pty_ready());
}

// -- keyboard input buffered before ready -------------------------------------

#[test]
fn on_key_buffered_while_pending() {
    let mut app = make_app();
    app.on_key(press_key(KeyCode::Char('a')));
    match &app.pty_lifecycle {
        PtyLifecycleState::Pending { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert_eq!(buffer[0], b"a");
        }
        other => panic!("Expected Pending, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn on_key_buffered_while_attached() {
    let mut app = make_app();
    app.dispatch_pty(PtyIntent::Attach);
    app.on_key(press_key(KeyCode::Char('x')));
    match &app.pty_lifecycle {
        PtyLifecycleState::Attached { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert_eq!(buffer[0], b"x");
        }
        other => panic!("Expected Attached, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn on_paste_buffered_while_not_ready() {
    let mut app = make_app();
    app.dispatch_pty(PtyIntent::Attach);
    app.on_paste("hello");
    match &app.pty_lifecycle {
        PtyLifecycleState::Attached { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert!(String::from_utf8_lossy(&buffer[0]).contains("hello"));
        }
        other => panic!("Expected Attached, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn on_image_paste_buffered_while_not_ready() {
    let mut app = make_app();
    app.dispatch_pty(PtyIntent::Attach);
    app.on_image_paste("data:image/png;base64,abc");
    match &app.pty_lifecycle {
        PtyLifecycleState::Attached { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert!(String::from_utf8_lossy(&buffer[0]).contains("data:image/png"));
        }
        other => panic!("Expected Attached, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn send_input_buffers_while_not_ready() {
    let mut app = make_app();
    app.dispatch_pty(PtyIntent::Attach);
    app.send_input(b"--resume");
    match &app.pty_lifecycle {
        PtyLifecycleState::Attached { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert_eq!(buffer[0], b"--resume");
        }
        other => panic!("Expected Attached, got {:?}", std::mem::discriminant(other)),
    }
}
