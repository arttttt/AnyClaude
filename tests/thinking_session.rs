//! Tests for ThinkingSession facade and TransformerRegistry public API.
//!
//! These tests cover the new API surface introduced by pipeline isolation:
//! - `begin_request()` atomicity and session stability
//! - `ThinkingSession::filter()` / `register_from_response()` lifecycle
//! - `notify_backend_switch()` invalidation
//! - Concurrent session safety

use anyclaude::metrics::DebugLogger;
use anyclaude::proxy::thinking::TransformerRegistry;
use std::sync::Arc;

fn make_registry() -> Arc<TransformerRegistry> {
    Arc::new(TransformerRegistry::new())
}

fn make_logger() -> Arc<DebugLogger> {
    Arc::new(DebugLogger::new(Default::default()))
}

/// Helper: create a response body with a thinking block.
fn response_with_thinking(signature: &str, thinking_text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "content": [
            {
                "type": "thinking",
                "thinking": thinking_text,
                "signature": signature,
            },
            {
                "type": "text",
                "text": "Hello"
            }
        ]
    })).unwrap()
}

/// Helper: create a request body referencing thinking blocks.
fn request_with_thinking(signature: &str, thinking_text: &str) -> serde_json::Value {
    serde_json::json!({
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "thinking",
                        "thinking": thinking_text,
                        "signature": signature,
                    },
                    {
                        "type": "text",
                        "text": "Hello"
                    }
                ]
            },
            {
                "role": "user",
                "content": "Continue"
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// begin_request() basics
// ---------------------------------------------------------------------------

#[test]
fn begin_request_returns_session() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);
    // Session should be usable (Debug trait works)
    let debug = format!("{:?}", session);
    assert!(debug.contains("ThinkingSession"));
    assert!(debug.contains("session_id"));
}

#[test]
fn begin_request_same_backend_stable_session() {
    let reg = make_registry();
    let logger = make_logger();

    let s1 = reg.begin_request("claude", logger.clone());
    let s2 = reg.begin_request("claude", logger.clone());
    let s3 = reg.begin_request("claude", logger);

    // Same backend → same session_id (no increment)
    let d1 = format!("{:?}", s1);
    let d2 = format!("{:?}", s2);
    let d3 = format!("{:?}", s3);
    assert_eq!(d1, d2);
    assert_eq!(d2, d3);
}

#[test]
fn begin_request_different_backend_increments_session() {
    let reg = make_registry();
    let logger = make_logger();

    let s1 = reg.begin_request("claude", logger.clone());
    let s2 = reg.begin_request("glm", logger);

    // Different backend → different session_id
    let d1 = format!("{:?}", s1);
    let d2 = format!("{:?}", s2);
    assert_ne!(d1, d2);
}

// ---------------------------------------------------------------------------
// filter() with no registered blocks
// ---------------------------------------------------------------------------

#[test]
fn filter_empty_body_returns_zero() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    let mut body = serde_json::json!({"messages": []});
    let filtered = session.filter(&mut body);
    assert_eq!(filtered, 0);
}

#[test]
fn filter_no_thinking_blocks_returns_zero() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    let mut body = serde_json::json!({
        "messages": [
            {"role": "user", "content": "Hello"},
            {"role": "assistant", "content": [{"type": "text", "text": "Hi"}]}
        ]
    });
    let filtered = session.filter(&mut body);
    assert_eq!(filtered, 0);
}

#[test]
fn filter_removes_unregistered_thinking_blocks() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    // Request with thinking blocks that were never registered
    let mut body = request_with_thinking("fake-sig-123", "some thinking");
    let filtered = session.filter(&mut body);
    assert!(filtered > 0);

    // The thinking block should be removed from the body
    let messages = body["messages"].as_array().unwrap();
    let assistant_content = messages[0]["content"].as_array().unwrap();
    assert!(
        assistant_content.iter().all(|c| c["type"] != "thinking"),
        "Thinking blocks should be removed"
    );
}

// ---------------------------------------------------------------------------
// register → filter lifecycle
// ---------------------------------------------------------------------------

#[test]
fn registered_blocks_survive_filter() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    // Simulate: backend responds with thinking blocks
    let response = response_with_thinking("valid-sig-abc", "deep thoughts");
    session.register_from_response(&response);

    // Now filter a request that references the same blocks
    let mut body = request_with_thinking("valid-sig-abc", "deep thoughts");
    let filtered = session.filter(&mut body);
    assert_eq!(filtered, 0, "Registered blocks should not be filtered");

    // Thinking block should still be in the body
    let messages = body["messages"].as_array().unwrap();
    let assistant_content = messages[0]["content"].as_array().unwrap();
    assert!(
        assistant_content.iter().any(|c| c["type"] == "thinking"),
        "Registered thinking block should survive filter"
    );
}

#[test]
fn register_then_switch_backend_invalidates_blocks() {
    let reg = make_registry();
    let logger = make_logger();

    // Register blocks on "claude" backend
    let session1 = reg.begin_request("claude", logger.clone());
    let response = response_with_thinking("claude-sig", "thinking on claude");
    session1.register_from_response(&response);

    let stats = reg.thinking_cache_stats();
    assert!(stats.total > 0, "Should have registered blocks");

    // Switch backend via IPC
    reg.notify_backend_switch("glm");

    // New session on "glm" — old blocks should be filtered
    let session2 = reg.begin_request("glm", logger);
    let mut body = request_with_thinking("claude-sig", "thinking on claude");
    let filtered = session2.filter(&mut body);
    assert!(filtered > 0, "Blocks from old backend should be filtered out");
}

// ---------------------------------------------------------------------------
// notify_backend_switch
// ---------------------------------------------------------------------------

#[test]
fn notify_backend_switch_same_backend_is_idempotent() {
    let reg = make_registry();
    let logger = make_logger();

    let s1 = reg.begin_request("claude", logger.clone());
    reg.notify_backend_switch("claude"); // same backend
    let s2 = reg.begin_request("claude", logger);

    let d1 = format!("{:?}", s1);
    let d2 = format!("{:?}", s2);
    assert_eq!(d1, d2, "Same backend switch should not change session");
}

#[test]
fn notify_backend_switch_different_backend_changes_session() {
    let reg = make_registry();
    let logger = make_logger();

    let s1 = reg.begin_request("claude", logger.clone());
    reg.notify_backend_switch("glm"); // different backend
    let s2 = reg.begin_request("glm", logger);

    let d1 = format!("{:?}", s1);
    let d2 = format!("{:?}", s2);
    assert_ne!(d1, d2, "Different backend should change session");
}

// ---------------------------------------------------------------------------
// Cache stats
// ---------------------------------------------------------------------------

#[test]
fn cache_stats_empty_on_new_registry() {
    let reg = make_registry();
    let stats = reg.thinking_cache_stats();
    assert_eq!(stats.total, 0);
}

#[test]
fn cache_stats_increments_after_register() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    let before = reg.thinking_cache_stats();
    session.register_from_response(&response_with_thinking("sig-1", "thought"));
    let after = reg.thinking_cache_stats();

    assert!(after.total > before.total, "Cache should grow after registration");
}

// ---------------------------------------------------------------------------
// Concurrent sessions don't corrupt state
// ---------------------------------------------------------------------------

#[test]
fn concurrent_begin_request_is_safe() {
    let reg = make_registry();
    let logger = make_logger();

    let mut handles = vec![];
    for i in 0..50 {
        let reg = reg.clone();
        let logger = logger.clone();
        handles.push(std::thread::spawn(move || {
            // Alternate between two backends
            let backend = if i % 2 == 0 { "claude" } else { "glm" };
            let session = reg.begin_request(backend, logger);
            // Exercise the session
            let mut body = serde_json::json!({"messages": []});
            session.filter(&mut body);
        }));
    }

    for handle in handles {
        handle.join().expect("Thread should not panic");
    }
}

#[test]
fn concurrent_register_and_filter() {
    let reg = make_registry();
    let logger = make_logger();

    // Pre-register some blocks
    let session = reg.begin_request("claude", logger.clone());
    for i in 0..10 {
        let response = response_with_thinking(
            &format!("sig-{}", i),
            &format!("thought {}", i),
        );
        session.register_from_response(&response);
    }

    let mut handles = vec![];
    for _ in 0..20 {
        let reg = reg.clone();
        let logger = logger.clone();
        handles.push(std::thread::spawn(move || {
            let session = reg.begin_request("claude", logger);
            let mut body = request_with_thinking("sig-0", "thought 0");
            session.filter(&mut body);
        }));
    }

    for handle in handles {
        handle.join().expect("Thread should not panic");
    }

    let stats = reg.thinking_cache_stats();
    assert!(stats.total > 0, "Registry should still have blocks after concurrent access");
}

// ---------------------------------------------------------------------------
// Edge: session captured before concurrent backend switch
// ---------------------------------------------------------------------------

#[test]
fn captured_session_stable_during_concurrent_switch() {
    let reg = make_registry();
    let logger = make_logger();

    // Session captured for "claude"
    let session = reg.begin_request("claude", logger.clone());
    let response = response_with_thinking("captured-sig", "stable thought");
    session.register_from_response(&response);

    // Another thread switches backend
    let reg_clone = reg.clone();
    std::thread::spawn(move || {
        reg_clone.notify_backend_switch("glm");
    }).join().unwrap();

    // Original session should still work — it has captured session_id
    // filter operates on the block's session vs request's session
    // After a backend switch, old blocks ARE invalidated even for the
    // original session holder — this is by design (safety over convenience).
    let mut body = request_with_thinking("captured-sig", "stable thought");
    let filtered = session.filter(&mut body);
    // After backend switch, blocks are invalidated
    assert!(filtered > 0, "Blocks should be invalidated after backend switch");
}

// ---------------------------------------------------------------------------
// Edge: register_from_response with malformed/empty data
// ---------------------------------------------------------------------------

#[test]
fn register_from_response_empty_body() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    // Should not panic
    session.register_from_response(b"");
    assert_eq!(reg.thinking_cache_stats().total, 0);
}

#[test]
fn register_from_response_invalid_json() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    // Should not panic
    session.register_from_response(b"not json at all");
    assert_eq!(reg.thinking_cache_stats().total, 0);
}

#[test]
fn register_from_response_no_thinking_blocks() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    let response = serde_json::to_vec(&serde_json::json!({
        "content": [{"type": "text", "text": "Hello"}]
    })).unwrap();
    session.register_from_response(&response);
    assert_eq!(reg.thinking_cache_stats().total, 0);
}

// ---------------------------------------------------------------------------
// SSE thinking block registration
// ---------------------------------------------------------------------------

fn make_sse_events(thinking_text: &str) -> Vec<anyclaude::sse::SseEvent> {
    vec![
        anyclaude::sse::SseEvent {
            event_type: "content_block_start".to_string(),
            data: serde_json::json!({
                "index": 0,
                "content_block": {"type": "thinking", "thinking": ""}
            }),
        },
        anyclaude::sse::SseEvent {
            event_type: "content_block_delta".to_string(),
            data: serde_json::json!({
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": thinking_text}
            }),
        },
        anyclaude::sse::SseEvent {
            event_type: "content_block_stop".to_string(),
            data: serde_json::json!({"index": 0}),
        },
    ]
}

#[test]
fn register_from_sse_adds_thinking_block_to_cache() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    assert_eq!(reg.thinking_cache_stats().total, 0);
    session.register_from_sse(&make_sse_events("SSE thinking content"));
    assert_eq!(reg.thinking_cache_stats().total, 1);
}

#[test]
fn register_from_sse_block_survives_filter() {
    let reg = make_registry();
    let logger = make_logger();

    // Register via SSE
    let session1 = reg.begin_request("claude", logger.clone());
    session1.register_from_sse(&make_sse_events("SSE thought"));

    // Filter request containing the same thinking text
    let session2 = reg.begin_request("claude", logger);
    let mut body = request_with_thinking("sig", "SSE thought");
    let filtered = session2.filter(&mut body);
    assert_eq!(filtered, 0, "SSE-registered block should survive filter");
}

#[test]
fn register_from_sse_empty_events() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    session.register_from_sse(&[]);
    assert_eq!(reg.thinking_cache_stats().total, 0);
}

#[test]
fn register_from_sse_malformed_events_no_panic() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    let malformed = vec![
        // Missing content_block field
        anyclaude::sse::SseEvent {
            event_type: "content_block_start".to_string(),
            data: serde_json::json!({"index": 0}),
        },
        // Delta without prior start
        anyclaude::sse::SseEvent {
            event_type: "content_block_delta".to_string(),
            data: serde_json::json!({
                "index": 99,
                "delta": {"type": "thinking_delta", "thinking": "orphan"}
            }),
        },
        // Stop without prior start
        anyclaude::sse::SseEvent {
            event_type: "content_block_stop".to_string(),
            data: serde_json::json!({"index": 99}),
        },
        // Unknown event type
        anyclaude::sse::SseEvent {
            event_type: "unknown_event".to_string(),
            data: serde_json::json!({"foo": "bar"}),
        },
        // Empty data
        anyclaude::sse::SseEvent {
            event_type: "content_block_start".to_string(),
            data: serde_json::json!(null),
        },
    ];

    // Should not panic
    session.register_from_sse(&malformed);
    assert_eq!(reg.thinking_cache_stats().total, 0);
}

#[test]
fn register_from_sse_multiple_blocks() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    let events = vec![
        // Block 0
        anyclaude::sse::SseEvent {
            event_type: "content_block_start".to_string(),
            data: serde_json::json!({"index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
        },
        anyclaude::sse::SseEvent {
            event_type: "content_block_delta".to_string(),
            data: serde_json::json!({"index": 0, "delta": {"type": "thinking_delta", "thinking": "first thought"}}),
        },
        anyclaude::sse::SseEvent {
            event_type: "content_block_stop".to_string(),
            data: serde_json::json!({"index": 0}),
        },
        // Block 1 (text, should be skipped)
        anyclaude::sse::SseEvent {
            event_type: "content_block_start".to_string(),
            data: serde_json::json!({"index": 1, "content_block": {"type": "text", "text": ""}}),
        },
        // Block 2 (another thinking)
        anyclaude::sse::SseEvent {
            event_type: "content_block_start".to_string(),
            data: serde_json::json!({"index": 2, "content_block": {"type": "thinking", "thinking": ""}}),
        },
        anyclaude::sse::SseEvent {
            event_type: "content_block_delta".to_string(),
            data: serde_json::json!({"index": 2, "delta": {"type": "thinking_delta", "thinking": "second thought"}}),
        },
        anyclaude::sse::SseEvent {
            event_type: "content_block_stop".to_string(),
            data: serde_json::json!({"index": 2}),
        },
    ];

    session.register_from_sse(&events);
    assert_eq!(reg.thinking_cache_stats().total, 2, "Should register 2 thinking blocks, skip text block");
}

// ---------------------------------------------------------------------------
// Edge: filter with non-object body
// ---------------------------------------------------------------------------

#[test]
fn filter_non_object_body() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    let mut body = serde_json::json!("just a string");
    let filtered = session.filter(&mut body);
    assert_eq!(filtered, 0);
}

#[test]
fn filter_null_body() {
    let reg = make_registry();
    let logger = make_logger();
    let session = reg.begin_request("claude", logger);

    let mut body = serde_json::json!(null);
    let filtered = session.filter(&mut body);
    assert_eq!(filtered, 0);
}
