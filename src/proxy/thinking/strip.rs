//! Strip transformer - removes thinking blocks entirely.

use async_trait::async_trait;
use serde_json::Value;

use super::context::{TransformContext, TransformResult, TransformStats};
use super::error::TransformError;
use super::traits::ThinkingTransformer;

// ============================================================================
// Public functions for stripping thinking blocks
// ============================================================================

/// Strip all thinking and redacted_thinking blocks from messages.
///
/// Returns the number of blocks removed.
///
/// This function is used by both `StripTransformer` and `SummarizeTransformer`
/// to remove thinking blocks from the request body.
pub fn strip_thinking_blocks(body: &mut Value) -> u32 {
    let Some(messages) = body.get_mut("messages").and_then(|v| v.as_array_mut()) else {
        return 0;
    };

    let mut stripped_count = 0u32;

    for message in messages.iter_mut() {
        let Some(content) = message.get_mut("content").and_then(|v| v.as_array_mut()) else {
            continue;
        };

        let before_len = content.len();

        // Remove all thinking and redacted_thinking blocks
        content.retain(|item| {
            let item_type = item.get("type").and_then(|t| t.as_str());
            !matches!(item_type, Some("thinking") | Some("redacted_thinking"))
        });

        stripped_count += (before_len - content.len()) as u32;
    }

    stripped_count
}

/// Remove the context_management field from the request body.
///
/// This field is used by Claude to manage thinking blocks, but becomes
/// invalid after we remove them. Should be called after stripping thinking blocks.
pub fn remove_context_management(body: &mut Value) -> bool {
    if let Some(obj) = body.as_object_mut() {
        if obj.remove("context_management").is_some() {
            tracing::debug!("Removed context_management field after stripping thinking blocks");
            return true;
        }
    }
    false
}

// ============================================================================
// StripTransformer
// ============================================================================

/// Transformer that strips (removes) all thinking blocks from requests.
///
/// This is the simplest and most compatible mode. It completely removes
/// thinking blocks from the message history, which:
/// - Prevents context accumulation
/// - Works with any backend
/// - Loses thinking context between turns
pub struct StripTransformer;

#[async_trait]
impl ThinkingTransformer for StripTransformer {
    fn name(&self) -> &'static str {
        "strip"
    }

    async fn transform_request(
        &self,
        body: &mut Value,
        _context: &TransformContext,
    ) -> Result<TransformResult, TransformError> {
        let mut stats = TransformStats::default();

        stats.stripped_count = strip_thinking_blocks(body);

        // Remove context_management field if we modified anything
        if stats.stripped_count > 0 {
            remove_context_management(body);
        }

        Ok(TransformResult::with_stats(stats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_context() -> TransformContext {
        TransformContext::new("test-backend", "test-request-123")
    }

    #[tokio::test]
    async fn strips_thinking_blocks() {
        let transformer = StripTransformer;
        let mut body = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "Let me think...", "signature": "abc123"},
                    {"type": "text", "text": "Hello!"}
                ]
            }]
        });

        let result = transformer
            .transform_request(&mut body, &make_context())
            .await
            .unwrap();

        assert!(result.changed);
        assert_eq!(result.stats.stripped_count, 1);

        // Check that thinking block is removed
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }

    #[tokio::test]
    async fn strips_redacted_thinking_blocks() {
        let transformer = StripTransformer;
        let mut body = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "redacted_thinking", "data": "encrypted..."},
                    {"type": "text", "text": "Result"}
                ]
            }]
        });

        let result = transformer
            .transform_request(&mut body, &make_context())
            .await
            .unwrap();

        assert!(result.changed);
        assert_eq!(result.stats.stripped_count, 1);

        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
    }

    #[tokio::test]
    async fn no_change_when_no_thinking() {
        let transformer = StripTransformer;
        let mut body = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Hello!"}
                ]
            }]
        });

        let result = transformer
            .transform_request(&mut body, &make_context())
            .await
            .unwrap();

        assert!(!result.changed);
        assert_eq!(result.stats.stripped_count, 0);
    }

    #[tokio::test]
    async fn handles_multiple_messages() {
        let transformer = StripTransformer;
        let mut body = json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "thinking": "thought 1", "signature": "sig1"},
                        {"type": "text", "text": "response 1"}
                    ]
                },
                {
                    "role": "user",
                    "content": "next question"
                },
                {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "thinking": "thought 2", "signature": "sig2"},
                        {"type": "text", "text": "response 2"}
                    ]
                }
            ]
        });

        let result = transformer
            .transform_request(&mut body, &make_context())
            .await
            .unwrap();

        assert!(result.changed);
        assert_eq!(result.stats.stripped_count, 2);
    }

    #[tokio::test]
    async fn removes_context_management_field() {
        let transformer = StripTransformer;
        let mut body = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "...", "signature": "..."},
                    {"type": "text", "text": "Hello"}
                ]
            }],
            "context_management": {
                "some": "config"
            }
        });

        let _ = transformer
            .transform_request(&mut body, &make_context())
            .await
            .unwrap();

        assert!(body.get("context_management").is_none());
    }

    #[tokio::test]
    async fn handles_empty_messages() {
        let transformer = StripTransformer;
        let mut body = json!({
            "messages": []
        });

        let result = transformer
            .transform_request(&mut body, &make_context())
            .await
            .unwrap();

        assert!(!result.changed);
    }

    #[tokio::test]
    async fn handles_missing_messages() {
        let transformer = StripTransformer;
        let mut body = json!({
            "model": "claude-3"
        });

        let result = transformer
            .transform_request(&mut body, &make_context())
            .await
            .unwrap();

        assert!(!result.changed);
    }
}
