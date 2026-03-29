mod common;

use std::collections::VecDeque;

use anyclaude::ui::pty::{PtyActor, PtyIntent, PtyLifecycleState};
use mvi::Store;

fn store() -> Store<PtyActor> {
    Store::new(PtyActor, |_| {})
}

fn store_with(state: PtyLifecycleState) -> Store<PtyActor> {
    Store::with_state(state, PtyActor, |_| {})
}

#[test]
fn pending_attach_transitions_to_attached() {
    let mut store = store();
    store.dispatch(PtyIntent::Attach);
    assert!(matches!(store.state(), PtyLifecycleState::Attached { buffer } if buffer.is_empty()));
}

#[test]
fn attached_attach_preserves_buffer() {
    let mut buf = VecDeque::new();
    buf.push_back(b"hello".to_vec());
    let mut store = store_with(PtyLifecycleState::Attached { buffer: buf });
    store.dispatch(PtyIntent::Attach);
    match store.state() {
        PtyLifecycleState::Attached { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert_eq!(buffer[0], b"hello");
        }
        _ => panic!("Expected Attached"),
    }
}

#[test]
fn ready_attach_stays_ready() {
    let mut store = store_with(PtyLifecycleState::Ready);
    store.dispatch(PtyIntent::Attach);
    assert!(matches!(store.state(), PtyLifecycleState::Ready));
}

#[test]
fn attached_got_output_transitions_to_ready() {
    let mut store = store_with(PtyLifecycleState::Attached {
        buffer: VecDeque::new(),
    });
    store.dispatch(PtyIntent::GotOutput);
    assert!(matches!(store.state(), PtyLifecycleState::Ready));
}

#[test]
fn pending_got_output_is_noop() {
    let mut store = store();
    store.dispatch(PtyIntent::GotOutput);
    assert!(matches!(store.state(), PtyLifecycleState::Pending { .. }));
}

#[test]
fn ready_got_output_is_noop() {
    let mut store = store_with(PtyLifecycleState::Ready);
    store.dispatch(PtyIntent::GotOutput);
    assert!(matches!(store.state(), PtyLifecycleState::Ready));
}

#[test]
fn pending_buffer_input_appends() {
    let mut store = store();
    store.dispatch(PtyIntent::BufferInput {
        bytes: b"data".to_vec(),
    });
    match store.state() {
        PtyLifecycleState::Pending { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert_eq!(buffer[0], b"data");
        }
        _ => panic!("Expected Pending"),
    }
}

#[test]
fn attached_buffer_input_appends() {
    let mut store = store_with(PtyLifecycleState::Attached {
        buffer: VecDeque::new(),
    });
    store.dispatch(PtyIntent::BufferInput {
        bytes: b"data".to_vec(),
    });
    match store.state() {
        PtyLifecycleState::Attached { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert_eq!(buffer[0], b"data");
        }
        _ => panic!("Expected Attached"),
    }
}

#[test]
fn ready_buffer_input_is_noop() {
    let mut store = store_with(PtyLifecycleState::Ready);
    store.dispatch(PtyIntent::BufferInput {
        bytes: b"data".to_vec(),
    });
    assert!(matches!(store.state(), PtyLifecycleState::Ready));
}

#[test]
fn multiple_buffer_inputs_accumulate() {
    let mut store = store();
    store.dispatch(PtyIntent::BufferInput {
        bytes: b"first".to_vec(),
    });
    store.dispatch(PtyIntent::BufferInput {
        bytes: b"second".to_vec(),
    });
    store.dispatch(PtyIntent::BufferInput {
        bytes: b"third".to_vec(),
    });
    match store.state() {
        PtyLifecycleState::Pending { buffer } => {
            assert_eq!(buffer.len(), 3);
            assert_eq!(buffer[0], b"first");
            assert_eq!(buffer[1], b"second");
            assert_eq!(buffer[2], b"third");
        }
        _ => panic!("Expected Pending"),
    }
}

#[test]
fn pending_detach_transitions_to_restarting() {
    let mut store = store();
    store.dispatch(PtyIntent::Detach);
    assert!(matches!(store.state(), PtyLifecycleState::Restarting));
}

#[test]
fn attached_detach_transitions_to_restarting() {
    let mut buf = VecDeque::new();
    buf.push_back(b"pending-data".to_vec());
    let mut store = store_with(PtyLifecycleState::Attached { buffer: buf });
    store.dispatch(PtyIntent::Detach);
    assert!(matches!(store.state(), PtyLifecycleState::Restarting));
}

#[test]
fn ready_detach_transitions_to_restarting() {
    let mut store = store_with(PtyLifecycleState::Ready);
    store.dispatch(PtyIntent::Detach);
    assert!(matches!(store.state(), PtyLifecycleState::Restarting));
}

#[test]
fn restarting_detach_stays_restarting() {
    let mut store = store_with(PtyLifecycleState::Restarting);
    store.dispatch(PtyIntent::Detach);
    assert!(matches!(store.state(), PtyLifecycleState::Restarting));
}

#[test]
fn restarting_attach_transitions_to_attached_empty_buffer() {
    let mut store = store_with(PtyLifecycleState::Restarting);
    store.dispatch(PtyIntent::Attach);
    match store.state() {
        PtyLifecycleState::Attached { buffer } => {
            assert!(buffer.is_empty(), "Buffer should be empty after restart attach");
        }
        other => panic!("Expected Attached, got {:?}", other),
    }
}

#[test]
fn restarting_buffer_input_drops_input() {
    let mut store = store_with(PtyLifecycleState::Restarting);
    store.dispatch(PtyIntent::BufferInput {
        bytes: b"dropped".to_vec(),
    });
    assert!(matches!(store.state(), PtyLifecycleState::Restarting));
}

#[test]
fn restarting_got_output_is_noop() {
    let mut store = store_with(PtyLifecycleState::Restarting);
    store.dispatch(PtyIntent::GotOutput);
    assert!(matches!(store.state(), PtyLifecycleState::Restarting));
}

#[test]
fn restarting_spawn_failed_transitions_to_pending() {
    let mut store = store_with(PtyLifecycleState::Restarting);
    store.dispatch(PtyIntent::SpawnFailed);
    match store.state() {
        PtyLifecycleState::Pending { buffer } => {
            assert!(buffer.is_empty());
        }
        other => panic!("Expected Pending, got {:?}", other),
    }
}

#[test]
fn pending_spawn_failed_is_noop() {
    let mut buf = VecDeque::new();
    buf.push_back(b"keep".to_vec());
    let mut store = store_with(PtyLifecycleState::Pending { buffer: buf });
    store.dispatch(PtyIntent::SpawnFailed);
    match store.state() {
        PtyLifecycleState::Pending { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert_eq!(buffer[0], b"keep");
        }
        _ => panic!("Expected Pending"),
    }
}

#[test]
fn ready_spawn_failed_is_noop() {
    let mut store = store_with(PtyLifecycleState::Ready);
    store.dispatch(PtyIntent::SpawnFailed);
    assert!(matches!(store.state(), PtyLifecycleState::Ready));
}

#[test]
fn is_restarting_returns_true_for_restarting() {
    assert!(PtyLifecycleState::Restarting.is_restarting());
}

#[test]
fn is_restarting_returns_false_for_other_states() {
    assert!(!PtyLifecycleState::Ready.is_restarting());
    assert!(!PtyLifecycleState::Pending { buffer: VecDeque::new() }.is_restarting());
    assert!(!PtyLifecycleState::Attached { buffer: VecDeque::new() }.is_restarting());
}

#[test]
fn full_restart_lifecycle_detach_attach_ready() {
    let mut store = store_with(PtyLifecycleState::Ready);
    store.dispatch(PtyIntent::Detach);
    assert!(matches!(store.state(), PtyLifecycleState::Restarting));
    store.dispatch(PtyIntent::BufferInput {
        bytes: b"ignored".to_vec(),
    });
    assert!(matches!(store.state(), PtyLifecycleState::Restarting));
    store.dispatch(PtyIntent::Attach);
    assert!(matches!(store.state(), PtyLifecycleState::Attached { ref buffer } if buffer.is_empty()));
    store.dispatch(PtyIntent::GotOutput);
    assert!(matches!(store.state(), PtyLifecycleState::Ready));
}

#[test]
fn restart_with_spawn_failure_then_recovery() {
    let mut store = store_with(PtyLifecycleState::Ready);
    store.dispatch(PtyIntent::Detach);
    assert!(matches!(store.state(), PtyLifecycleState::Restarting));
    store.dispatch(PtyIntent::SpawnFailed);
    assert!(matches!(store.state(), PtyLifecycleState::Pending { ref buffer } if buffer.is_empty()));
    store.dispatch(PtyIntent::Attach);
    assert!(matches!(store.state(), PtyLifecycleState::Attached { ref buffer } if buffer.is_empty()));
}
