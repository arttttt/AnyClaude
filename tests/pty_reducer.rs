mod common;

use std::collections::VecDeque;

use mvi::Reducer;
use anyclaude::ui::pty::{PtyIntent, PtyLifecycleState, PtyReducer};

#[test]
fn pending_attach_transitions_to_attached() {
    let state = PtyLifecycleState::Pending {
        buffer: VecDeque::new(),
    };
    let new = PtyReducer::reduce(state, PtyIntent::Attach);
    assert!(matches!(new, PtyLifecycleState::Attached { buffer } if buffer.is_empty()));
}

#[test]
fn attached_attach_preserves_buffer() {
    let mut buf = VecDeque::new();
    buf.push_back(b"hello".to_vec());
    let state = PtyLifecycleState::Attached { buffer: buf };

    let new = PtyReducer::reduce(state, PtyIntent::Attach);
    match new {
        PtyLifecycleState::Attached { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert_eq!(buffer[0], b"hello");
        }
        _ => panic!("Expected Attached"),
    }
}

#[test]
fn ready_attach_stays_ready() {
    let new = PtyReducer::reduce(PtyLifecycleState::Ready, PtyIntent::Attach);
    assert!(matches!(new, PtyLifecycleState::Ready));
}

#[test]
fn attached_got_output_transitions_to_ready() {
    let state = PtyLifecycleState::Attached {
        buffer: VecDeque::new(),
    };
    let new = PtyReducer::reduce(state, PtyIntent::GotOutput);
    assert!(matches!(new, PtyLifecycleState::Ready));
}

#[test]
fn pending_got_output_is_noop() {
    let state = PtyLifecycleState::Pending {
        buffer: VecDeque::new(),
    };
    let new = PtyReducer::reduce(state, PtyIntent::GotOutput);
    assert!(matches!(new, PtyLifecycleState::Pending { .. }));
}

#[test]
fn ready_got_output_is_noop() {
    let new = PtyReducer::reduce(PtyLifecycleState::Ready, PtyIntent::GotOutput);
    assert!(matches!(new, PtyLifecycleState::Ready));
}

#[test]
fn pending_buffer_input_appends() {
    let state = PtyLifecycleState::Pending {
        buffer: VecDeque::new(),
    };
    let new = PtyReducer::reduce(
        state,
        PtyIntent::BufferInput {
            bytes: b"data".to_vec(),
        },
    );
    match new {
        PtyLifecycleState::Pending { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert_eq!(buffer[0], b"data");
        }
        _ => panic!("Expected Pending"),
    }
}

#[test]
fn attached_buffer_input_appends() {
    let state = PtyLifecycleState::Attached {
        buffer: VecDeque::new(),
    };
    let new = PtyReducer::reduce(
        state,
        PtyIntent::BufferInput {
            bytes: b"data".to_vec(),
        },
    );
    match new {
        PtyLifecycleState::Attached { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert_eq!(buffer[0], b"data");
        }
        _ => panic!("Expected Attached"),
    }
}

#[test]
fn ready_buffer_input_is_noop() {
    let new = PtyReducer::reduce(
        PtyLifecycleState::Ready,
        PtyIntent::BufferInput {
            bytes: b"data".to_vec(),
        },
    );
    assert!(matches!(new, PtyLifecycleState::Ready));
}

#[test]
fn multiple_buffer_inputs_accumulate() {
    let state = PtyLifecycleState::Pending {
        buffer: VecDeque::new(),
    };

    let state = PtyReducer::reduce(
        state,
        PtyIntent::BufferInput {
            bytes: b"first".to_vec(),
        },
    );
    let state = PtyReducer::reduce(
        state,
        PtyIntent::BufferInput {
            bytes: b"second".to_vec(),
        },
    );
    let state = PtyReducer::reduce(
        state,
        PtyIntent::BufferInput {
            bytes: b"third".to_vec(),
        },
    );

    match state {
        PtyLifecycleState::Pending { buffer } => {
            assert_eq!(buffer.len(), 3);
            assert_eq!(buffer[0], b"first");
            assert_eq!(buffer[1], b"second");
            assert_eq!(buffer[2], b"third");
        }
        _ => panic!("Expected Pending"),
    }
}

// --- Detach intent (any state → Restarting) ---

#[test]
fn pending_detach_transitions_to_restarting() {
    let state = PtyLifecycleState::Pending {
        buffer: VecDeque::new(),
    };
    let new = PtyReducer::reduce(state, PtyIntent::Detach);
    assert!(matches!(new, PtyLifecycleState::Restarting));
}

#[test]
fn attached_detach_transitions_to_restarting() {
    let mut buf = VecDeque::new();
    buf.push_back(b"pending-data".to_vec());
    let state = PtyLifecycleState::Attached { buffer: buf };
    let new = PtyReducer::reduce(state, PtyIntent::Detach);
    assert!(matches!(new, PtyLifecycleState::Restarting));
}

#[test]
fn ready_detach_transitions_to_restarting() {
    let new = PtyReducer::reduce(PtyLifecycleState::Ready, PtyIntent::Detach);
    assert!(matches!(new, PtyLifecycleState::Restarting));
}

#[test]
fn restarting_detach_stays_restarting() {
    let new = PtyReducer::reduce(PtyLifecycleState::Restarting, PtyIntent::Detach);
    assert!(matches!(new, PtyLifecycleState::Restarting));
}

// --- Restarting state behavior ---

#[test]
fn restarting_attach_transitions_to_attached_empty_buffer() {
    let new = PtyReducer::reduce(PtyLifecycleState::Restarting, PtyIntent::Attach);
    match new {
        PtyLifecycleState::Attached { buffer } => {
            assert!(buffer.is_empty(), "Buffer should be empty after restart attach");
        }
        _ => panic!("Expected Attached, got {:?}", new),
    }
}

#[test]
fn restarting_buffer_input_drops_input() {
    let new = PtyReducer::reduce(
        PtyLifecycleState::Restarting,
        PtyIntent::BufferInput {
            bytes: b"dropped".to_vec(),
        },
    );
    assert!(matches!(new, PtyLifecycleState::Restarting));
}

#[test]
fn restarting_got_output_is_noop() {
    let new = PtyReducer::reduce(PtyLifecycleState::Restarting, PtyIntent::GotOutput);
    assert!(matches!(new, PtyLifecycleState::Restarting));
}

// --- SpawnFailed intent ---

#[test]
fn restarting_spawn_failed_transitions_to_pending() {
    let new = PtyReducer::reduce(PtyLifecycleState::Restarting, PtyIntent::SpawnFailed);
    match new {
        PtyLifecycleState::Pending { buffer } => {
            assert!(buffer.is_empty());
        }
        _ => panic!("Expected Pending, got {:?}", new),
    }
}

#[test]
fn pending_spawn_failed_is_noop() {
    let mut buf = VecDeque::new();
    buf.push_back(b"keep".to_vec());
    let state = PtyLifecycleState::Pending { buffer: buf };
    let new = PtyReducer::reduce(state, PtyIntent::SpawnFailed);
    match new {
        PtyLifecycleState::Pending { buffer } => {
            assert_eq!(buffer.len(), 1);
            assert_eq!(buffer[0], b"keep");
        }
        _ => panic!("Expected Pending"),
    }
}

#[test]
fn ready_spawn_failed_is_noop() {
    let new = PtyReducer::reduce(PtyLifecycleState::Ready, PtyIntent::SpawnFailed);
    assert!(matches!(new, PtyLifecycleState::Ready));
}

// --- is_restarting helper ---

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

// --- Full restart lifecycle ---

#[test]
fn full_restart_lifecycle_detach_attach_ready() {
    // Start from Ready (normal running state)
    let state = PtyLifecycleState::Ready;

    // Settings apply triggers Detach
    let state = PtyReducer::reduce(state, PtyIntent::Detach);
    assert!(matches!(state, PtyLifecycleState::Restarting));

    // Input during restart is dropped
    let state = PtyReducer::reduce(
        state,
        PtyIntent::BufferInput {
            bytes: b"ignored".to_vec(),
        },
    );
    assert!(matches!(state, PtyLifecycleState::Restarting));

    // New PTY spawned and attached
    let state = PtyReducer::reduce(state, PtyIntent::Attach);
    assert!(matches!(state, PtyLifecycleState::Attached { ref buffer } if buffer.is_empty()));

    // First output from new PTY
    let state = PtyReducer::reduce(state, PtyIntent::GotOutput);
    assert!(matches!(state, PtyLifecycleState::Ready));
}

#[test]
fn restart_with_spawn_failure_then_recovery() {
    let state = PtyLifecycleState::Ready;

    // Detach for restart
    let state = PtyReducer::reduce(state, PtyIntent::Detach);
    assert!(matches!(state, PtyLifecycleState::Restarting));

    // Spawn fails
    let state = PtyReducer::reduce(state, PtyIntent::SpawnFailed);
    assert!(matches!(state, PtyLifecycleState::Pending { ref buffer } if buffer.is_empty()));

    // Eventually re-attached (e.g. retry or user action)
    let state = PtyReducer::reduce(state, PtyIntent::Attach);
    assert!(matches!(state, PtyLifecycleState::Attached { ref buffer } if buffer.is_empty()));
}
