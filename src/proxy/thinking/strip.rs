//! Strip transformer - removes thinking blocks entirely.

use async_trait::async_trait;
use serde_json::Value;

use super::context::{TransformContext, TransformResult, TransformStats};
use super::error::TransformError;
use super::traits::ThinkingTransformer;

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

        let Some(messages) = body.get_mut("messages").and_then(|v| v.as_array_mut()) else {
            return Ok(TransformResult::unchanged());
        };

        for message in messages.iter_mut() {
            let Some(content) = message.get_mut("content").and_then(|v| v.as_array_mut()) else {
                continue;
            };

            // Count thinking blocks before removal
            let before_len = content.len();

            // Remove all thinking and redacted_thinking blocks
            content.retain(|item| {
                let item_type = item.get("type").and_then(|t| t.as_str());
                !matches!(item_type, Some("thinking") | Some("redacted_thinking"))
            });

            let removed = before_len - content.len();
            stats.stripped_count += removed as u32;
        }

        // Remove context_management field if we modified anything
        // This field is used by Claude to manage thinking blocks,
        // but becomes invalid after we remove them
        if stats.stripped_count > 0 {
            if let Some(obj) = body.as_object_mut() {
                if obj.remove("context_management").is_some() {
                    tracing::debug!("Removed context_management field after stripping thinking blocks");
                }
            }
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
