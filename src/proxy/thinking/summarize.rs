//! Summarize transformer - keeps thinking native, summarizes on backend switch.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::SummarizeConfig;
use crate::metrics::DebugLogger;

use super::context::{TransformContext, TransformResult, TransformStats};
use super::error::TransformError;
use super::summarizer::SummarizerClient;
use super::traits::ThinkingTransformer;

/// Last messages and response, kept together under one lock for atomicity.
struct LastContext {
    messages: Option<Vec<Value>>,
    response: Option<String>,
}

/// Transformer that summarizes session history when switching backends.
///
/// This mode:
/// - Saves messages from each request for potential summarization
/// - Captures the assistant's response when streaming completes
/// - When backend switch is triggered, calls LLM to summarize the session
/// - Prepends the summary to the first user message after switch
/// - Strips thinking blocks (they're captured in the summary)
pub struct SummarizeTransformer {
    /// Last messages and response, protected by a single lock to ensure
    /// on_backend_switch reads a consistent pair.
    last_context: RwLock<LastContext>,
    /// Summary waiting to be prepended to the next request.
    pending_summary: RwLock<Option<String>>,
    /// Configuration for summarization.
    config: SummarizeConfig,
    /// Client for calling the summarization API.
    summarizer: Option<SummarizerClient>,
    /// Debug logger for verbose logging.
    debug_logger: Option<Arc<DebugLogger>>,
}

impl SummarizeTransformer {
    /// Create a new SummarizeTransformer with the given configuration.
    pub fn new(config: SummarizeConfig, debug_logger: Option<Arc<DebugLogger>>) -> Self {
        let summarizer = SummarizerClient::new(config.clone(), debug_logger.clone());

        if summarizer.is_none() {
            tracing::warn!(
                "SummarizeTransformer created without API key - summarization will not work"
            );
        }

        Self {
            last_context: RwLock::new(LastContext {
                messages: None,
                response: None,
            }),
            pending_summary: RwLock::new(None),
            config,
            summarizer,
            debug_logger,
        }
    }

    /// Get the current configuration.
    pub fn config(&self) -> &SummarizeConfig {
        &self.config
    }

    /// Check if summarization is available (API key configured).
    pub fn is_summarization_available(&self) -> bool {
        self.summarizer.is_some()
    }
}

/// Prepend a summary to the first user message in the request body.
///
/// The summary is wrapped in context tags and prepended to the user's content.
/// Handles both string content and array content formats.
///
/// Returns `true` if a user message was found and modified.
pub fn prepend_summary_to_user_message(body: &mut Value, summary: &str) -> bool {
    let Some(messages) = body.get_mut("messages").and_then(|v| v.as_array_mut()) else {
        return false;
    };

    // Find first user message
    let Some(user_msg) = messages.iter_mut().find(|m| {
        m.get("role").and_then(|r| r.as_str()) == Some("user")
    }) else {
        return false;
    };

    let context_block = format!(
        "[CONTEXT FROM PREVIOUS SESSION]\n{}\n[/CONTEXT FROM PREVIOUS SESSION]\n\n",
        summary
    );

    // Handle content as string
    if let Some(content_str) = user_msg.get("content").and_then(|c| c.as_str()) {
        let new_content = format!("{}{}", context_block, content_str);
        user_msg["content"] = Value::String(new_content);
        return true;
    }

    // Handle content as array
    if let Some(content_arr) = user_msg.get_mut("content").and_then(|c| c.as_array_mut()) {
        // Find first text block and prepend to it
        for item in content_arr.iter_mut() {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    let new_text = format!("{}{}", context_block, text);
                    item["text"] = Value::String(new_text);
                    return true;
                }
            }
        }

        // No text block found, insert one at the beginning
        content_arr.insert(0, serde_json::json!({
            "type": "text",
            "text": context_block.trim_end()
        }));
        return true;
    }

    false
}

#[async_trait]
impl ThinkingTransformer for SummarizeTransformer {
    fn name(&self) -> &'static str {
        "summarize"
    }

    async fn transform_request(
        &self,
        body: &mut Value,
        context: &TransformContext,
    ) -> Result<TransformResult, TransformError> {
        let mut stats = TransformStats::default();

        // 1. Save messages for potential future summarization
        // Only save for real chat completion requests (/v1/messages endpoint),
        // not for count_tokens or other auxiliary requests that could overwrite
        // the real conversation with minimal/test messages.
        // Also, only save if the new message count is >= existing count to preserve
        // maximum context (internal requests like title generation have fewer messages).
        if context.is_chat_completion() {
            if let Some(messages) = body.get("messages").and_then(|m| m.as_array()) {
                let mut ctx = self.last_context.write().await;
                let current_count = ctx.messages.as_ref().map(|m| m.len()).unwrap_or(0);

                if messages.len() >= current_count {
                    ctx.messages = Some(messages.clone());
                    tracing::trace!(
                        message_count = messages.len(),
                        "Saved messages for potential summarization"
                    );
                } else {
                    tracing::trace!(
                        new_count = messages.len(),
                        existing_count = current_count,
                        "Skipped saving messages - fewer than existing"
                    );
                }
            }
        }

        // 2. Check if we have a pending summary to prepend
        {
            let mut pending = self.pending_summary.write().await;
            if let Some(summary) = pending.take() {
                if prepend_summary_to_user_message(body, &summary) {
                    stats.summarized_count = 1;
                    tracing::info!("Prepended session summary to first user message");

                    // Log the modified request body for debugging
                    if let Some(logger) = &self.debug_logger {
                        let body_json = serde_json::to_string_pretty(body)
                            .unwrap_or_else(|_| "<serialization error>".to_string());
                        logger.log_auxiliary_full(
                            "summarize-prepend",
                            None,
                            None,
                            Some(&format!("summary_len={}", summary.len())),
                            None,
                            Some(&body_json),
                            None,
                        );
                    }
                } else {
                    tracing::warn!("Had pending summary but no user message to prepend to");
                }
            }
        }

        Ok(TransformResult::with_stats(stats))
    }

    async fn on_backend_switch(
        &self,
        from_backend: &str,
        to_backend: &str,
        _body: &mut Value,
    ) -> Result<(), TransformError> {
        tracing::info!(
            from = %from_backend,
            to = %to_backend,
            "Backend switch detected, starting summarization"
        );

        // Get the summarizer client
        let summarizer = match &self.summarizer {
            Some(s) => s,
            None => {
                tracing::warn!("Summarization not available - no API key configured");
                return Err(TransformError::BackendUnavailable(
                    "Summarizer not configured - missing API key".to_string(),
                ));
            }
        };

        // Get the messages and response atomically under a single lock
        let (messages, last_response) = {
            let ctx = self.last_context.read().await;
            (ctx.messages.clone(), ctx.response.clone())
        };

        let mut messages = match messages {
            Some(m) if !m.is_empty() => m,
            _ => {
                tracing::info!("No messages to summarize, skipping");
                return Ok(());
            }
        };

        // Append the last assistant response if we have one
        // This captures the response that was streamed but not yet included in a request
        if let Some(response_text) = last_response.as_ref() {
            if !response_text.is_empty() {
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": response_text
                }));
                tracing::debug!(
                    response_len = response_text.len(),
                    "Appended last assistant response to messages for summarization"
                );
            }
        }

        tracing::debug!(
            message_count = messages.len(),
            "Summarizing session messages"
        );

        // Call the summarization API
        let summary = summarizer.summarize(&messages).await.map_err(|e| {
            tracing::error!(error = %e, "Summarization API call failed");
            TransformError::from(e)
        })?;

        tracing::info!(
            summary_len = summary.len(),
            "Summarization complete, storing for next request"
        );

        // Store the summary for the next request
        {
            let mut pending = self.pending_summary.write().await;
            *pending = Some(summary);
        }

        Ok(())
    }

    async fn on_response_complete(&self, response_text: String) {
        if response_text.is_empty() {
            return;
        }

        tracing::debug!(
            response_len = response_text.len(),
            "Captured assistant response for potential summarization"
        );

        let mut ctx = self.last_context.write().await;
        ctx.response = Some(response_text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_context() -> TransformContext {
        TransformContext::new("test-backend", "test-request-123", "/v1/messages")
    }

    fn make_config() -> SummarizeConfig {
        SummarizeConfig::default()
    }

    #[tokio::test]
    async fn name_returns_summarize() {
        let transformer = SummarizeTransformer::new(make_config(), None);
        assert_eq!(transformer.name(), "summarize");
    }

    #[tokio::test]
    async fn saves_messages_on_transform() {
        let transformer = SummarizeTransformer::new(make_config(), None);
        let mut body = json!({
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there!"}
            ]
        });

        let _ = transformer
            .transform_request(&mut body, &make_context())
            .await
            .unwrap();

        // Verify messages were saved
        let ctx = transformer.last_context.read().await;
        assert!(ctx.messages.is_some());
        assert_eq!(ctx.messages.as_ref().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn does_not_strip_thinking_blocks_eagerly() {
        // SummarizeTransformer no longer strips thinking blocks eagerly.
        // Stripping happens lazily on 400 retry in upstream.rs
        let transformer = SummarizeTransformer::new(make_config(), None);
        let mut body = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "...", "signature": "..."},
                    {"type": "text", "text": "Hello!"}
                ]
            }]
        });

        let result = transformer
            .transform_request(&mut body, &make_context())
            .await
            .unwrap();

        // No stripping should occur
        assert!(!result.changed);
        assert_eq!(result.stats.stripped_count, 0);

        // Thinking blocks should still be there
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "thinking");
    }

    #[tokio::test]
    async fn config_accessible() {
        let config = SummarizeConfig {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("test-key".to_string()),
            model: "test-model".to_string(),
            max_tokens: 100,
        };
        let transformer = SummarizeTransformer::new(config, None);

        assert_eq!(transformer.config().model, "test-model");
        assert_eq!(transformer.config().max_tokens, 100);
        assert_eq!(transformer.config().base_url, "https://api.example.com");
    }

    // ========================================================================
    // Prepend summary tests
    // ========================================================================

    #[test]
    fn prepend_to_string_content() {
        let mut body = json!({
            "messages": [
                {"role": "user", "content": "Hello"}
            ]
        });

        let result = prepend_summary_to_user_message(&mut body, "Session summary here");

        assert!(result);
        let content = body["messages"][0]["content"].as_str().unwrap();
        assert!(content.starts_with("[CONTEXT FROM PREVIOUS SESSION]"));
        assert!(content.contains("Session summary here"));
        assert!(content.contains("[/CONTEXT FROM PREVIOUS SESSION]"));
        assert!(content.ends_with("Hello"));
    }

    #[test]
    fn prepend_to_array_content_with_text() {
        let mut body = json!({
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Hello"}
                    ]
                }
            ]
        });

        let result = prepend_summary_to_user_message(&mut body, "Summary");

        assert!(result);
        let text = body["messages"][0]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("[CONTEXT FROM PREVIOUS SESSION]"));
        assert!(text.contains("Summary"));
        assert!(text.ends_with("Hello"));
    }

    #[test]
    fn prepend_to_array_content_without_text() {
        let mut body = json!({
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "image", "source": {"type": "base64", "data": "..."}}
                    ]
                }
            ]
        });

        let result = prepend_summary_to_user_message(&mut body, "Summary");

        assert!(result);
        // Should insert text block at the beginning
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert!(content[0]["text"].as_str().unwrap().contains("Summary"));
    }

    #[test]
    fn prepend_no_user_message() {
        let mut body = json!({
            "messages": [
                {"role": "assistant", "content": "Hi"}
            ]
        });

        let result = prepend_summary_to_user_message(&mut body, "Summary");
        assert!(!result);
    }

    #[test]
    fn prepend_no_messages() {
        let mut body = json!({"model": "claude-3"});

        let result = prepend_summary_to_user_message(&mut body, "Summary");
        assert!(!result);
    }

    #[tokio::test]
    async fn pending_summary_is_prepended_and_cleared() {
        let transformer = SummarizeTransformer::new(make_config(), None);

        // Set pending summary
        {
            let mut pending = transformer.pending_summary.write().await;
            *pending = Some("Test summary".to_string());
        }

        let mut body = json!({
            "messages": [
                {"role": "user", "content": "Hello"}
            ]
        });

        let result = transformer
            .transform_request(&mut body, &make_context())
            .await
            .unwrap();

        // Summary should be prepended
        assert_eq!(result.stats.summarized_count, 1);
        let content = body["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("Test summary"));

        // Pending should be cleared
        let pending = transformer.pending_summary.read().await;
        assert!(pending.is_none());
    }

    // ========================================================================
    // on_backend_switch tests
    // ========================================================================

    #[tokio::test]
    async fn on_backend_switch_fails_without_summarizer() {
        // Config without API key - summarizer won't be created
        let config = make_config();
        let transformer = SummarizeTransformer::new(config, None);

        // Simulate having messages saved
        {
            let mut ctx = transformer.last_context.write().await;
            ctx.messages = Some(vec![json!({"role": "user", "content": "Hello"})]);
        }

        let mut body = json!({});
        let result = transformer
            .on_backend_switch("backend-a", "backend-b", &mut body)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not available") || err.contains("not configured"));
    }

    #[tokio::test]
    async fn on_backend_switch_skips_when_no_messages() {
        let config = SummarizeConfig {
            api_key: Some("test-key".to_string()),
            ..make_config()
        };
        let transformer = SummarizeTransformer::new(config, None);

        // No messages saved - should skip without error
        let mut body = json!({});
        let result = transformer
            .on_backend_switch("backend-a", "backend-b", &mut body)
            .await;

        assert!(result.is_ok());

        // No pending summary should be set
        let pending = transformer.pending_summary.read().await;
        assert!(pending.is_none());
    }

    #[tokio::test]
    async fn is_summarization_available_reflects_api_key() {
        // Without API key
        let config = make_config();
        let transformer = SummarizeTransformer::new(config, None);
        assert!(!transformer.is_summarization_available());

        // With API key
        let config_with_key = SummarizeConfig {
            api_key: Some("test-key".to_string()),
            ..make_config()
        };
        let transformer_with_key = SummarizeTransformer::new(config_with_key, None);
        assert!(transformer_with_key.is_summarization_available());
    }
}
