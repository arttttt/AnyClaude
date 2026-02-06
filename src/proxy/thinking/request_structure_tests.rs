//! Tests for edge cases in request filtering that may cause 500 errors
//!
//! These tests verify that filtering doesn't break request structure

#[cfg(test)]
mod request_structure_tests {
    use crate::proxy::thinking::ThinkingRegistry;
    use serde_json::json;

    /// Test: Empty content after filtering thinking blocks
    /// This simulates when an assistant message had only thinking blocks
    /// and all were removed. Anthropic API requires non-empty content.
    #[test]
    fn test_empty_content_after_filtering() {
        let mut registry = ThinkingRegistry::new();
        registry.on_backend_switch("anthropic-opus");

        // Request with assistant message containing ONLY thinking blocks
        // (no text content) - all thinking blocks are from old session
        let mut request = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "Old thought 1", "signature": "sig1"},
                    {"type": "thinking", "thinking": "Old thought 2", "signature": "sig2"}
                ]
            }]
        });

        // All blocks should be removed (not in cache)
        let removed = registry.filter_request(&mut request);
        assert_eq!(removed, 2, "Should remove 2 old thinking blocks");

        // Check if content is now empty
        let content = request["messages"][0]["content"].as_array().unwrap();
        println!("Content after filtering: {:?}", content);

        // This is the BUG: content is empty [], which causes 500 error
        assert!(
            content.is_empty(),
            "BUG: Content became empty after filtering!"
        );

        // In real scenario, Anthropic would return 500 error for empty content
        // The fix should ensure content is never empty after filtering
    }

    /// Test: Assistant message with thinking + text - only thinking removed
    #[test]
    fn test_mixed_content_after_filtering() {
        let mut registry = ThinkingRegistry::new();
        registry.on_backend_switch("anthropic-opus");

        // Request with both thinking and text - only thinking is old
        let mut request = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "Old thought", "signature": "sig1"},
                    {"type": "text", "text": "This should remain"}
                ]
            }]
        });

        let removed = registry.filter_request(&mut request);
        assert_eq!(removed, 1, "Should remove 1 old thinking block");

        // Text should remain
        let content = request["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1, "Text block should remain");
        assert_eq!(content[0]["type"], "text");
    }

    /// Test: Multiple assistant messages with varying content
    #[test]
    fn test_multiple_assistant_messages() {
        let mut registry = ThinkingRegistry::new();
        registry.on_backend_switch("anthropic-opus");

        // Register one block
        let response = json!({
            "content": [{
                "type": "thinking",
                "thinking": "Valid thought",
                "signature": "sig1"
            }]
        });
        registry.register_from_response(&serde_json::to_vec(&response).unwrap(), registry.current_session());

        // Request with multiple assistant messages
        let mut request = json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "thinking": "Old thought", "signature": "old"},
                        {"type": "text", "text": "Message 1"}
                    ]
                },
                {
                    "role": "user",
                    "content": "Next"
                },
                {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "thinking": "Valid thought", "signature": "sig1"},
                        {"type": "text", "text": "Message 2"}
                    ]
                }
            ]
        });

        let removed = registry.filter_request(&mut request);
        assert_eq!(removed, 1, "Should remove only the old thought");

        // Check both assistant messages
        let msg1_content = request["messages"][0]["content"].as_array().unwrap();
        let msg3_content = request["messages"][2]["content"].as_array().unwrap();

        assert_eq!(msg1_content.len(), 1, "Msg1: text should remain");
        assert_eq!(msg3_content.len(), 2, "Msg3: both blocks should remain");
    }

    /// Test: String content (not array) should be preserved
    #[test]
    fn test_string_content_preserved() {
        let mut registry = ThinkingRegistry::new();
        registry.on_backend_switch("anthropic-opus");

        // Request with string content (simple text, not array of blocks)
        let mut request = json!({
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there"}  // String, not array
            ]
        });

        let removed = registry.filter_request(&mut request);
        assert_eq!(removed, 0);

        // Content should remain as string
        let content = &request["messages"][1]["content"];
        assert!(content.is_string(), "String content should remain string");
        assert_eq!(content.as_str().unwrap(), "Hi there");
    }

    /// Test: Null content handling
    #[test]
    fn test_null_content_handling() {
        let mut registry = ThinkingRegistry::new();
        registry.on_backend_switch("anthropic-opus");

        // Request with null content
        let mut request = json!({
            "messages": [
                {"role": "assistant", "content": null}
            ]
        });

        // Should not panic
        let removed = registry.filter_request(&mut request);
        assert_eq!(removed, 0);
    }

    /// Test: Malformed thinking block without content field
    #[test]
    fn test_malformed_thinking_block() {
        let mut registry = ThinkingRegistry::new();
        registry.on_backend_switch("anthropic-opus");

        // Request with thinking block missing "thinking" field
        let mut request = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "signature": "sig"},  // Missing "thinking" field!
                    {"type": "text", "text": "Response"}
                ]
            }]
        });

        // Should remove malformed block
        let removed = registry.filter_request(&mut request);
        assert_eq!(removed, 1, "Should remove malformed thinking block");

        // Text should remain
        let content = request["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }

    /// Test: Very long thinking content that gets truncated in hash
    /// This ensures hash calculation handles long content correctly
    #[test]
    fn test_long_thinking_content_hashing() {
        let mut registry = ThinkingRegistry::new();
        registry.on_backend_switch("anthropic-opus");

        // Create very long thinking content (> 1000 chars)
        let long_thought = "A".repeat(2000);

        // Register from response
        let response = json!({
            "content": [{
                "type": "thinking",
                "thinking": &long_thought,
                "signature": "sig"
            }]
        });
        registry.register_from_response(&serde_json::to_vec(&response).unwrap(), registry.current_session());

        // Request with same long content
        let mut request = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": &long_thought, "signature": "sig"},
                    {"type": "text", "text": "Response"}
                ]
            }]
        });

        let removed = registry.filter_request(&mut request);
        assert_eq!(removed, 0, "Long content should hash correctly");
    }

    /// Test: Special characters in thinking content
    #[test]
    fn test_special_characters_in_thinking() {
        let mut registry = ThinkingRegistry::new();
        registry.on_backend_switch("anthropic-opus");

        let special_thought = "Test with \"quotes\", \\backslashes\\, and emoji 🎉";

        // Register
        let response = json!({
            "content": [{
                "type": "thinking",
                "thinking": special_thought,
                "signature": "sig"
            }]
        });
        registry.register_from_response(&serde_json::to_vec(&response).unwrap(), registry.current_session());

        // Request with same content
        let mut request = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": special_thought, "signature": "sig"},
                    {"type": "text", "text": "Response"}
                ]
            }]
        });

        let removed = registry.filter_request(&mut request);
        assert_eq!(removed, 0, "Special characters should be handled correctly");
    }
}
