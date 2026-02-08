mod common;

use std::collections::VecDeque;

use anyclaude::ui::pty::PtyLifecycleState;

#[test]
fn default_is_pending() {
    let state = PtyLifecycleState::default();
    assert!(matches!(state, PtyLifecycleState::Pending { buffer } if buffer.is_empty()));
}

#[test]
fn is_ready_check() {
    assert!(!PtyLifecycleState::default().is_ready());
    assert!(!PtyLifecycleState::Attached { buffer: VecDeque::new() }.is_ready());
    assert!(PtyLifecycleState::Ready.is_ready());
}

#[test]
fn is_buffering_check() {
    assert!(PtyLifecycleState::default().is_buffering());
    assert!(PtyLifecycleState::Attached { buffer: VecDeque::new() }.is_buffering());
    assert!(!PtyLifecycleState::Ready.is_buffering());
}
