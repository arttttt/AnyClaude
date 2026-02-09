//! Tests for App PTY lifecycle state machine and input buffering.

mod common;

use anyclaude::ui::pty::{PtyIntent, PtyLifecycleState};
use common::*;
use crossterm::event::KeyCode;

// -- Restarting state tests --------------------------------------------------

#[test]
fn on_key_dropped_while_restarting() {
    let mut app = make_app();
    // Transition to Ready first
    app.dispatch_pty(PtyIntent::Attach);
    app.dispatch_pty(PtyIntent::GotOutput);
    assert!(app.is_pty_ready());

    // Detach to enter Restarting state
    app.dispatch_pty(PtyIntent::Detach);
    assert!(app.pty_lifecycle.is_restarting());

    // Input should be dropped (not buffered)
    app.on_key(press_key(KeyCode::Char('a')));
    assert!(app.pty_lifecycle.is_restarting());

    // After restart, attach and verify no buffered input
    app.dispatch_pty(PtyIntent::Attach);
    match &app.pty_lifecycle {
        PtyLifecycleState::Attached { buffer } => {
            assert!(buffer.is_empty(), "Buffer should be empty after restart");
        }
        other => panic!("Expected Attached, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn detach_pty_transitions_to_restarting() {
    let mut app = make_app();
    // Start from Ready state
    app.dispatch_pty(PtyIntent::Attach);
    app.dispatch_pty(PtyIntent::GotOutput);
    assert!(app.is_pty_ready());

    // Detach should transition to Restarting
    app.detach_pty();
    assert!(app.pty_lifecycle.is_restarting());
}

#[test]
fn attach_after_restart_clears_buffer() {
    let mut app = make_app();

    // Build up buffer in Attached state
    app.dispatch_pty(PtyIntent::Attach);
    app.on_key(press_key(KeyCode::Char('x')));
    app.on_key(press_key(KeyCode::Char('y')));

    match &app.pty_lifecycle {
        PtyLifecycleState::Attached { buffer } => {
            assert_eq!(buffer.len(), 2);
        }
        other => panic!("Expected Attached, got {:?}", std::mem::discriminant(other)),
    }

    // Detach to Restarting
    app.dispatch_pty(PtyIntent::Detach);
    assert!(app.pty_lifecycle.is_restarting());

    // Attach after restart should have empty buffer
    app.dispatch_pty(PtyIntent::Attach);
    match &app.pty_lifecycle {
        PtyLifecycleState::Attached { buffer } => {
            assert!(buffer.is_empty(), "Buffer should be cleared after restart attach");
        }
        other => panic!("Expected Attached, got {:?}", std::mem::discriminant(other)),
    }
}

// -- Generation counter tests ------------------------------------------------

#[test]
fn next_pty_generation_increments_counter() {
    let mut app = make_app();

    // Initial generation is 0
    assert_eq!(app.pty_generation(), 0);

    // First increment
    let gen1 = app.next_pty_generation();
    assert_eq!(gen1, 1);
    assert_eq!(app.pty_generation(), 1);

    // Second increment
    let gen2 = app.next_pty_generation();
    assert_eq!(gen2, 2);
    assert_eq!(app.pty_generation(), 2);

    // Third increment
    let gen3 = app.next_pty_generation();
    assert_eq!(gen3, 3);
    assert_eq!(app.pty_generation(), 3);
}

#[test]
fn has_restarted_returns_true_after_restart() {
    let mut app = make_app();

    // Initially no restart
    assert!(!app.has_restarted());
    assert_eq!(app.pty_generation(), 0);

    // After first generation increment (simulating a restart)
    app.next_pty_generation();
    assert!(app.has_restarted());
    assert_eq!(app.pty_generation(), 1);

    // After more increments
    app.next_pty_generation();
    assert!(app.has_restarted());
    assert_eq!(app.pty_generation(), 2);
}

#[test]
fn has_restarted_returns_false_initially() {
    let app = make_app();
    assert!(!app.has_restarted());
    assert_eq!(app.pty_generation(), 0);
}

// -- Full restart cycle integration test -------------------------------------

#[test]
fn full_restart_cycle_with_generation_tracking() {
    let mut app = make_app();

    // Initial state
    assert_eq!(app.pty_generation(), 0);
    assert!(!app.has_restarted());

    // Simulate initial PTY spawn
    app.next_pty_generation();
    app.dispatch_pty(PtyIntent::Attach);
    app.dispatch_pty(PtyIntent::GotOutput);
    assert!(app.is_pty_ready());
    assert_eq!(app.pty_generation(), 1);
    assert!(app.has_restarted());

    // Simulate settings change triggering restart
    app.detach_pty();
    assert!(app.pty_lifecycle.is_restarting());

    // Simulate new PTY spawn after restart
    app.next_pty_generation();
    app.dispatch_pty(PtyIntent::Attach);
    app.dispatch_pty(PtyIntent::GotOutput);

    assert!(app.is_pty_ready());
    assert_eq!(app.pty_generation(), 2);
    assert!(app.has_restarted());
}

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
    app.on_image_paste(std::path::Path::new("/tmp/anyclaude/paste_123.png"));
    match &app.pty_lifecycle {
        PtyLifecycleState::Attached { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert!(String::from_utf8_lossy(&buffer[0]).contains("/tmp/anyclaude/paste_123.png"));
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
