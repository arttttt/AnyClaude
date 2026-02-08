//! Tests for ThinkingRegistry: block tracking, confirmation, cleanup, filtering.

mod common;

use anyclaude::proxy::thinking::{fast_hash, safe_suffix, safe_truncate, ThinkingRegistry};
use anyclaude::sse::parse_sse_events;
use serde_json::{json, Value};
use std::time::Duration;

// ========================================================================
// Helper functions
// ========================================================================

fn make_request_with_thinking(thoughts: &[&str]) -> Value {
    let content: Vec<Value> = thoughts
        .iter()
        .map(|t| {
            json!({
                "type": "thinking",
                "thinking": t,
                "signature": "test-sig"
            })
        })
        .chain(std::iter::once(json!({"type": "text", "text": "Hello"})))
        .collect();

    json!({
        "messages": [{
            "role": "assistant",
            "content": content
        }]
    })
}

fn make_response_with_thinking(thoughts: &[&str]) -> Vec<u8> {
    let content: Vec<Value> = thoughts
        .iter()
        .map(|t| {
            json!({
                "type": "thinking",
                "thinking": t,
                "signature": "test-sig"
            })
        })
        .collect();

    serde_json::to_vec(&json!({ "content": content })).unwrap()
}

fn make_request_without_thinking_but_with_history() -> Value {
    json!({
        "messages": [
            {
                "role": "user",
                "content": "Hello"
            },
            {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I'll help you with that."}
                ]
            },
            {
                "role": "user",
                "content": "Do something"
            }
        ]
    })
}

// ========================================================================
// Basic functionality tests
// ========================================================================

#[test]
fn test_new_registry() {
    let registry = ThinkingRegistry::new();
    assert_eq!(registry.current_session(), 0);
    assert_eq!(registry.current_backend(), "");
    assert_eq!(registry.block_count(), 0);
}

#[test]
fn test_backend_switch_increments_session() {
    let mut registry = ThinkingRegistry::new();

    registry.on_backend_switch("anthropic");
    assert_eq!(registry.current_session(), 1);
    assert_eq!(registry.current_backend(), "anthropic");

    registry.on_backend_switch("glm");
    assert_eq!(registry.current_session(), 2);
    assert_eq!(registry.current_backend(), "glm");

    // Same backend doesn't increment
    registry.on_backend_switch("glm");
    assert_eq!(registry.current_session(), 2);

    registry.on_backend_switch("anthropic");
    assert_eq!(registry.current_session(), 3);
}

#[test]
fn test_register_from_response() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let response = make_response_with_thinking(&["Thought A", "Thought B"]);
    registry.register_from_response(&response, registry.current_session());

    assert_eq!(registry.block_count(), 2);

    let stats = registry.cache_stats();
    assert_eq!(stats.unconfirmed, 2);
    assert_eq!(stats.confirmed, 0);
}

#[test]
fn test_register_from_sse_stream_full_flow() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let sse_stream = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Hello \"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"world\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n";

    let events = parse_sse_events(sse_stream);
    registry.register_from_sse_stream(&events, registry.current_session());
    assert_eq!(registry.block_count(), 1);

    // Verify the registered block matches the full accumulated text
    let hash = fast_hash("Hello world");
    assert!(registry.blocks.contains_key(&hash));
}

#[test]
fn test_register_from_sse_stream_redacted_thinking() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let sse_stream = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"encrypted-data-abc\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n";

    let events = parse_sse_events(sse_stream);
    registry.register_from_sse_stream(&events, registry.current_session());
    assert_eq!(registry.block_count(), 1);
}

#[test]
fn test_register_from_sse_stream_multiple_blocks() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let sse_stream = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Thought A\"}}\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\
data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Thought B\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
data: {\"type\":\"content_block_stop\",\"index\":1}\n\
data: {\"type\":\"content_block_stop\",\"index\":2}\n";

    let events = parse_sse_events(sse_stream);
    registry.register_from_sse_stream(&events, registry.current_session());
    assert_eq!(registry.block_count(), 2, "should register 2 thinking blocks");
}

#[test]
fn test_sse_stream_registered_blocks_match_request() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let sse_stream = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me analyze\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n";

    let events = parse_sse_events(sse_stream);
    registry.register_from_sse_stream(&events, registry.current_session());

    // Now filter a request containing the same thinking text
    let mut request = make_request_with_thinking(&["Let me analyze"]);
    let removed = registry.filter_request(&mut request);
    assert_eq!(removed, 0, "SSE-registered block should match request block");
}

#[test]
fn test_register_deduplication() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let response = make_response_with_thinking(&["Same thought"]);
    registry.register_from_response(&response, registry.current_session());
    registry.register_from_response(&response, registry.current_session());
    registry.register_from_response(&response, registry.current_session());

    // Should only have one entry
    assert_eq!(registry.block_count(), 1);
}

// ========================================================================
// Confirmation tests
// ========================================================================

#[test]
fn test_confirm_blocks_on_request() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    // Register block
    let response = make_response_with_thinking(&["Thought A"]);
    registry.register_from_response(&response, registry.current_session());
    assert_eq!(registry.cache_stats().unconfirmed, 1);

    // Send request with the block - should confirm it
    let mut request = make_request_with_thinking(&["Thought A"]);
    registry.filter_request(&mut request);

    assert_eq!(registry.cache_stats().confirmed, 1);
    assert_eq!(registry.cache_stats().unconfirmed, 0);
}

#[test]
fn test_confirm_only_current_session() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    // Register block in session 1
    let response = make_response_with_thinking(&["Thought A"]);
    registry.register_from_response(&response, registry.current_session());

    // Switch to session 2
    registry.on_backend_switch("glm");

    // Request with block from session 1 - should NOT confirm (different session)
    let mut request = make_request_with_thinking(&["Thought A"]);
    let removed = registry.filter_request(&mut request);

    // Block should be removed from request (old session)
    assert_eq!(removed, 1);
    // And removed from cache
    assert_eq!(registry.block_count(), 0);
}

// ========================================================================
// Cleanup tests - old session
// ========================================================================

#[test]
fn test_cleanup_removes_old_session_blocks() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    // Register blocks in session 1
    let response = make_response_with_thinking(&["Old thought"]);
    registry.register_from_response(&response, registry.current_session());
    assert_eq!(registry.block_count(), 1);

    // Switch to session 2
    registry.on_backend_switch("glm");

    // Process empty request - should cleanup old session blocks
    let mut request = json!({"messages": []});
    registry.filter_request(&mut request);

    assert_eq!(registry.block_count(), 0);
}

#[test]
fn test_cleanup_old_session_even_if_in_request() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    // Register block in session 1
    let response = make_response_with_thinking(&["Thought A"]);
    registry.register_from_response(&response, registry.current_session());

    // Switch to session 2
    registry.on_backend_switch("glm");

    // Request still has old block (CC hasn't updated yet)
    let mut request = make_request_with_thinking(&["Thought A"]);
    let removed = registry.filter_request(&mut request);

    // Block removed from request
    assert_eq!(removed, 1);
    // Block removed from cache
    assert_eq!(registry.block_count(), 0);
}

// ========================================================================
// Cleanup tests - confirmed unused
// ========================================================================

#[test]
fn test_cleanup_removes_confirmed_not_in_request() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    // Register and confirm blocks A and B
    let response = make_response_with_thinking(&["Thought A", "Thought B"]);
    registry.register_from_response(&response, registry.current_session());

    let mut request = make_request_with_thinking(&["Thought A", "Thought B"]);
    registry.filter_request(&mut request);
    assert_eq!(registry.cache_stats().confirmed, 2);

    // Next request only has A (B was truncated from context)
    let mut request = make_request_with_thinking(&["Thought A"]);
    registry.filter_request(&mut request);

    // B should be removed (confirmed but not in request)
    assert_eq!(registry.block_count(), 1);
    assert_eq!(registry.cache_stats().confirmed, 1);
}

#[test]
fn test_cleanup_keeps_confirmed_in_request() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    // Register and confirm block
    let response = make_response_with_thinking(&["Thought A"]);
    registry.register_from_response(&response, registry.current_session());

    // Multiple requests with same block
    for _ in 0..5 {
        let mut request = make_request_with_thinking(&["Thought A"]);
        registry.filter_request(&mut request);
    }

    // Block should still be there
    assert_eq!(registry.block_count(), 1);
    assert_eq!(registry.cache_stats().confirmed, 1);
}

// ========================================================================
// Cleanup tests - orphaned (unconfirmed + expired)
// ========================================================================

#[test]
fn test_cleanup_keeps_unconfirmed_within_threshold() {
    // Use very short threshold for testing
    let mut registry = ThinkingRegistry::with_orphan_threshold(Duration::from_secs(3600));
    registry.on_backend_switch("anthropic");

    // Register block (not confirmed yet)
    let response = make_response_with_thinking(&["Thought A"]);
    registry.register_from_response(&response, registry.current_session());

    // Request without the block (simulating empty first request)
    let mut request = json!({"messages": []});
    registry.filter_request(&mut request);

    // Block should still be there (within threshold)
    assert_eq!(registry.block_count(), 1);
    assert_eq!(registry.cache_stats().unconfirmed, 1);
}

#[test]
fn test_cleanup_removes_orphaned_after_threshold() {
    // Use zero threshold - any unconfirmed block not in request is removed
    let mut registry = ThinkingRegistry::with_orphan_threshold(Duration::ZERO);
    registry.on_backend_switch("anthropic");
    let session = registry.current_session();

    // Register two blocks: one will be in the request, one won't (orphan)
    let response = make_response_with_thinking(&["Thought A", "Thought B"]);
    registry.register_from_response(&response, session);
    assert_eq!(registry.block_count(), 2);

    // Request contains only "Thought B" — "Thought A" is orphaned.
    // request_hashes is non-empty so Rules 2/3 apply.
    let mut request = make_request_with_thinking(&["Thought B"]);
    registry.filter_request(&mut request);

    // "Thought A" should be removed (orphan, threshold=0), "Thought B" kept
    assert_eq!(registry.block_count(), 1);
}

// ========================================================================
// Filter request tests
// ========================================================================

#[test]
fn test_filter_removes_unregistered_blocks() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    // Request with block we never registered
    let mut request = make_request_with_thinking(&["Unknown thought"]);
    let removed = registry.filter_request(&mut request);

    assert_eq!(removed, 1);

    // Text block should remain
    let content = request["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
}

#[test]
fn test_filter_keeps_registered_blocks() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    // Register block
    let response = make_response_with_thinking(&["Known thought"]);
    registry.register_from_response(&response, registry.current_session());

    // Request with that block
    let mut request = make_request_with_thinking(&["Known thought"]);
    let removed = registry.filter_request(&mut request);

    assert_eq!(removed, 0);

    // Both blocks should remain
    let content = request["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2); // thinking + text
}

#[test]
fn test_filter_handles_redacted_thinking() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    // Register via response with redacted thinking
    let response = serde_json::to_vec(&json!({
        "content": [{
            "type": "redacted_thinking",
            "data": "encrypted-data-123"
        }]
    }))
    .unwrap();
    registry.register_from_response(&response, registry.current_session());

    // Request with same redacted thinking
    let mut request = json!({
        "messages": [{
            "role": "assistant",
            "content": [
                {"type": "redacted_thinking", "data": "encrypted-data-123"},
                {"type": "text", "text": "Hello"}
            ]
        }]
    });
    let removed = registry.filter_request(&mut request);

    assert_eq!(removed, 0);
}

#[test]
fn test_filter_multiple_messages() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    // Register only one thought
    let response = make_response_with_thinking(&["Known"]);
    registry.register_from_response(&response, registry.current_session());

    // Request with multiple messages, some known some unknown
    let mut request = json!({
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "Known", "signature": "s1"},
                    {"type": "text", "text": "Response 1"}
                ]
            },
            {
                "role": "user",
                "content": "Next question"
            },
            {
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "Unknown", "signature": "s2"},
                    {"type": "text", "text": "Response 2"}
                ]
            }
        ]
    });

    let removed = registry.filter_request(&mut request);
    assert_eq!(removed, 1); // Only "Unknown" removed

    // Verify structure
    let msg0_content = request["messages"][0]["content"].as_array().unwrap();
    assert_eq!(msg0_content.len(), 2); // thinking + text

    let msg2_content = request["messages"][2]["content"].as_array().unwrap();
    assert_eq!(msg2_content.len(), 1); // only text
}

// ========================================================================
// Full flow tests (positive scenarios)
// ========================================================================

#[test]
fn test_full_flow_normal_conversation() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let response1 = make_response_with_thinking(&["Analyzing the problem..."]);
    registry.register_from_response(&response1, registry.current_session());
    assert_eq!(registry.block_count(), 1);

    let mut request2 = make_request_with_thinking(&["Analyzing the problem..."]);
    let removed = registry.filter_request(&mut request2);
    assert_eq!(removed, 0);
    assert_eq!(registry.cache_stats().confirmed, 1);

    let response2 = make_response_with_thinking(&["Let me elaborate..."]);
    registry.register_from_response(&response2, registry.current_session());
    assert_eq!(registry.block_count(), 2);

    let mut request3 =
        make_request_with_thinking(&["Analyzing the problem...", "Let me elaborate..."]);
    let removed = registry.filter_request(&mut request3);
    assert_eq!(removed, 0);
    assert_eq!(registry.cache_stats().confirmed, 2);
}

#[test]
fn test_full_flow_context_truncation() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    for i in 1..=5 {
        let response = make_response_with_thinking(&[&format!("Thought {}", i)]);
        registry.register_from_response(&response, registry.current_session());

        let thoughts: Vec<String> = (1..=i).map(|j| format!("Thought {}", j)).collect();
        let thought_refs: Vec<&str> = thoughts.iter().map(|s| s.as_str()).collect();
        let mut request = make_request_with_thinking(&thought_refs);
        registry.filter_request(&mut request);
    }

    assert_eq!(registry.block_count(), 5);
    assert_eq!(registry.cache_stats().confirmed, 5);

    let mut request = make_request_with_thinking(&["Thought 4", "Thought 5"]);
    registry.filter_request(&mut request);

    assert_eq!(registry.block_count(), 2);
}

#[test]
fn test_full_flow_backend_switch() {
    let mut registry = ThinkingRegistry::new();

    registry.on_backend_switch("anthropic");
    let response1 = make_response_with_thinking(&["Anthropic thought"]);
    registry.register_from_response(&response1, registry.current_session());

    let mut request1 = make_request_with_thinking(&["Anthropic thought"]);
    registry.filter_request(&mut request1);
    assert_eq!(registry.cache_stats().confirmed, 1);

    registry.on_backend_switch("glm");

    let mut request2 = make_request_with_thinking(&["Anthropic thought"]);
    let removed = registry.filter_request(&mut request2);
    assert_eq!(removed, 1);
    assert_eq!(registry.block_count(), 0);

    let response2 = make_response_with_thinking(&["GLM thought"]);
    registry.register_from_response(&response2, registry.current_session());

    let mut request3 = make_request_with_thinking(&["GLM thought"]);
    let removed = registry.filter_request(&mut request3);
    assert_eq!(removed, 0);
    assert_eq!(registry.block_count(), 1);
}

#[test]
fn test_full_flow_rapid_backend_switches() {
    let mut registry = ThinkingRegistry::new();

    registry.on_backend_switch("a");
    registry.on_backend_switch("b");
    registry.on_backend_switch("c");
    registry.on_backend_switch("a");

    assert_eq!(registry.current_session(), 4);

    let response = make_response_with_thinking(&["New thought"]);
    registry.register_from_response(&response, registry.current_session());

    let mut request = make_request_with_thinking(&["New thought"]);
    let removed = registry.filter_request(&mut request);
    assert_eq!(removed, 0);
}

// ========================================================================
// Negative / edge case tests
// ========================================================================

#[test]
fn test_negative_empty_request() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let response = make_response_with_thinking(&["Thought"]);
    registry.register_from_response(&response, registry.current_session());

    let mut request = json!({"messages": []});
    let removed = registry.filter_request(&mut request);
    assert_eq!(removed, 0);
}

#[test]
fn test_negative_no_messages_field() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let mut request = json!({"model": "claude-3"});
    let removed = registry.filter_request(&mut request);
    assert_eq!(removed, 0);
}

#[test]
fn test_negative_string_content() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let mut request = json!({
        "messages": [
            {"role": "user", "content": "Hello"},
            {"role": "assistant", "content": "Hi there"}
        ]
    });
    let removed = registry.filter_request(&mut request);
    assert_eq!(removed, 0);
}

#[test]
fn test_negative_malformed_thinking_block() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let mut request = json!({
        "messages": [{
            "role": "assistant",
            "content": [
                {"type": "thinking"},
                {"type": "text", "text": "Hello"}
            ]
        }]
    });
    let removed = registry.filter_request(&mut request);
    assert_eq!(removed, 1);
}

#[test]
fn test_negative_unknown_block_type() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let mut request = json!({
        "messages": [{
            "role": "assistant",
            "content": [
                {"type": "image", "data": "base64..."},
                {"type": "text", "text": "Hello"}
            ]
        }]
    });
    let removed = registry.filter_request(&mut request);
    assert_eq!(removed, 0);

    let content = request["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
}

#[test]
fn test_negative_register_empty_response() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    registry.register_from_response(b"", registry.current_session());
    registry.register_from_response(b"{}", registry.current_session());
    registry.register_from_response(b"{\"content\": []}", registry.current_session());

    assert_eq!(registry.block_count(), 0);
}

#[test]
fn test_negative_register_invalid_json() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    registry.register_from_response(b"not json", registry.current_session());
    let events = parse_sse_events(b"data: not json\n");
    registry.register_from_sse_stream(&events, registry.current_session());

    assert_eq!(registry.block_count(), 0);
}

// ========================================================================
// Hash tests
// ========================================================================

#[test]
fn test_fast_hash_uniqueness() {
    let hash1 = fast_hash("Hello world");
    let hash2 = fast_hash("Hello world!");
    let hash3 = fast_hash("Hello world");

    assert_ne!(hash1, hash2);
    assert_eq!(hash1, hash3);
}

#[test]
fn test_fast_hash_long_content() {
    let short = "a".repeat(100);
    let long = "a".repeat(1000);

    let hash1 = fast_hash(&short);
    let hash2 = fast_hash(&long);

    assert_ne!(hash1, hash2);
}

#[test]
fn test_fast_hash_unicode() {
    let hash1 = fast_hash("Привет мир");
    let hash2 = fast_hash("Привет мир!");
    let hash3 = fast_hash("Привет мир");

    assert_ne!(hash1, hash2);
    assert_eq!(hash1, hash3);
}

#[test]
fn test_safe_truncate_unicode() {
    let s = "Привет"; // 12 bytes, 6 chars
    assert_eq!(safe_truncate(s, 100), s);
    assert_eq!(safe_truncate(s, 12), s);
    assert_eq!(safe_truncate(s, 11), "Приве"); // Can't cut in middle of char
    assert_eq!(safe_truncate(s, 2), "П");
    assert_eq!(safe_truncate(s, 1), "");
}

#[test]
fn test_safe_suffix_unicode() {
    let s = "Привет"; // 12 bytes, 6 chars
    assert_eq!(safe_suffix(s, 100), s);
    assert_eq!(safe_suffix(s, 12), s);
    assert_eq!(safe_suffix(s, 11), "ривет"); // Can't cut in middle of char
    assert_eq!(safe_suffix(s, 2), "т");
    assert_eq!(safe_suffix(s, 1), "");
}

#[test]
fn test_fast_hash_same_prefix_suffix_different_middle() {
    let prefix = "START_".repeat(50);
    let suffix = "_END".repeat(70);

    let content1 = format!("{}MIDDLE_A{}", prefix, suffix);
    let content2 = format!("{}MIDDLE_B{}", prefix, suffix);

    let hash1 = fast_hash(&content1);
    let hash2 = fast_hash(&content2);

    // These WILL collide - documenting expected behavior
    assert_eq!(hash1, hash2, "Known limitation: same prefix+suffix+length = same hash");
}

#[test]
fn test_fast_hash_same_prefix_different_suffix() {
    let prefix = "X".repeat(300);
    let content1 = format!("{}ENDING_AAA", prefix);
    let content2 = format!("{}ENDING_BBB", prefix);

    let hash1 = fast_hash(&content1);
    let hash2 = fast_hash(&content2);

    assert_ne!(hash1, hash2);
}

// ========================================================================
// Cache stats tests
// ========================================================================

#[test]
fn test_cache_stats() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");

    let response = make_response_with_thinking(&["A", "B", "C"]);
    registry.register_from_response(&response, registry.current_session());

    let stats = registry.cache_stats();
    assert_eq!(stats.total, 3);
    assert_eq!(stats.unconfirmed, 3);
    assert_eq!(stats.confirmed, 0);
    assert_eq!(stats.current_session, 3);
    assert_eq!(stats.old_session, 0);

    let mut request = make_request_with_thinking(&["A", "B"]);
    registry.filter_request(&mut request);

    let stats = registry.cache_stats();
    assert_eq!(stats.confirmed, 2);
    assert_eq!(stats.unconfirmed, 1);
}

// ========================================================================
// Haiku sub-request eviction bug tests
// ========================================================================

#[test]
fn test_haiku_subrequest_must_not_evict_confirmed_blocks() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");
    let session = registry.current_session();

    let response = make_response_with_thinking(&["Deep analysis of the problem"]);
    registry.register_from_response(&response, session);
    assert_eq!(registry.block_count(), 1);

    let mut opus_request = make_request_with_thinking(&["Deep analysis of the problem"]);
    let removed = registry.filter_request(&mut opus_request);
    assert_eq!(removed, 0, "Block should be kept (same session)");

    let stats = registry.cache_stats();
    assert_eq!(stats.confirmed, 1, "Block should be confirmed");
    assert_eq!(stats.total, 1);

    let mut haiku_request = make_request_without_thinking_but_with_history();
    registry.filter_request(&mut haiku_request);

    assert_eq!(
        registry.block_count(),
        1,
        "Haiku sub-request must NOT evict confirmed thinking blocks"
    );

    let mut next_opus_request = make_request_with_thinking(&["Deep analysis of the problem"]);
    let removed = registry.filter_request(&mut next_opus_request);
    assert_eq!(
        removed, 0,
        "Valid thinking block from current session should NOT be stripped"
    );
}

#[test]
fn test_multiple_blocks_survive_haiku_subrequest() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");
    let session = registry.current_session();

    for thought in &["Thought A", "Thought B", "Thought C"] {
        let response = make_response_with_thinking(&[thought]);
        registry.register_from_response(&response, session);
    }
    assert_eq!(registry.block_count(), 3);

    let mut opus_request =
        make_request_with_thinking(&["Thought A", "Thought B", "Thought C"]);
    let removed = registry.filter_request(&mut opus_request);
    assert_eq!(removed, 0);
    assert_eq!(registry.cache_stats().confirmed, 3);

    for _ in 0..5 {
        let mut haiku = make_request_without_thinking_but_with_history();
        registry.filter_request(&mut haiku);
    }

    assert_eq!(
        registry.block_count(),
        3,
        "All confirmed blocks must survive haiku sub-requests"
    );

    let mut next =
        make_request_with_thinking(&["Thought A", "Thought B", "Thought C"]);
    let removed = registry.filter_request(&mut next);
    assert_eq!(removed, 0, "No blocks should be stripped");
}

#[test]
fn test_interleaved_opus_haiku_workflow() {
    let mut registry = ThinkingRegistry::new();
    registry.on_backend_switch("anthropic");
    let session = registry.current_session();

    let resp1 = make_response_with_thinking(&["Planning step 1"]);
    registry.register_from_response(&resp1, session);

    let mut req1 = make_request_with_thinking(&["Planning step 1"]);
    registry.filter_request(&mut req1);
    assert_eq!(registry.cache_stats().confirmed, 1);

    let mut haiku1 = make_request_without_thinking_but_with_history();
    registry.filter_request(&mut haiku1);

    let mut haiku2 = make_request_without_thinking_but_with_history();
    registry.filter_request(&mut haiku2);

    let resp2 = make_response_with_thinking(&["Planning step 2"]);
    registry.register_from_response(&resp2, session);

    let mut req2 = make_request_with_thinking(&["Planning step 1", "Planning step 2"]);
    let removed = registry.filter_request(&mut req2);
    assert_eq!(
        removed, 0,
        "Both thinking blocks should be preserved after haiku sub-requests"
    );
    assert_eq!(registry.block_count(), 2);
}
