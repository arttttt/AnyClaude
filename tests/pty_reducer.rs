mod common;

use std::collections::VecDeque;

use anyclaude::ui::mvi::Reducer;
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
