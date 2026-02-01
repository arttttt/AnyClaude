//! Summarize transformer - keeps thinking native, summarizes on backend switch.

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::config::SummarizeConfig;

use super::context::{TransformContext, TransformResult, TransformStats};
use super::error::TransformError;
use super::strip::{remove_context_management, strip_thinking_blocks};
use super::traits::ThinkingTransformer;

/// Transformer that summarizes session history when switching backends.
///
/// This mode:
/// - Saves messages from each request for potential summarization
/// - When backend switch is triggered, calls LLM to summarize the session
/// - Prepends the summary to the first user message after switch
/// - Strips thinking blocks (they're captured in the summary)
pub struct SummarizeTransformer {
    /// Last messages seen, for summarization when switching backends.
    last_messages: RwLock<Option<Vec<Value>>>,
    /// Summary waiting to be prepended to the next request.
    pending_summary: RwLock<Option<String>>,
    /// Configuration for summarization.
    config: SummarizeConfig,
}

impl SummarizeTransformer {
    /// Create a new SummarizeTransformer with the given configuration.
    pub fn new(config: SummarizeConfig) -> Self {
        Self {
            last_messages: RwLock::new(None),
            pending_summary: RwLock::new(None),
            config,
        }
    }

    /// Get the current configuration.
    pub fn config(&self) -> &SummarizeConfig {
        &self.config
    }
}

#[async_trait]
impl ThinkingTransformer for SummarizeTransformer {
    fn name(&self) -> &'static str {
        "summarize"
    }

    async fn transform_request(
        &self,
        body: &mut Value,
        _context: &TransformContext,
    ) -> Result<TransformResult, TransformError> {
        let mut stats = TransformStats::default();

        // 1. Save messages for potential future summarization
        if let Some(messages) = body.get("messages").and_then(|m| m.as_array()) {
            let mut last_messages = self.last_messages.write().await;
            *last_messages = Some(messages.clone());
            tracing::trace!(
                message_count = messages.len(),
                "Saved messages for potential summarization"
            );
        }

        // 2. Check if we have a pending summary to prepend
        // (This will be implemented in Phase 2.4)
        {
            let pending = self.pending_summary.read().await;
            if pending.is_some() {
                stats.summarized_count = 1;
                // TODO: Phase 2.4 - prepend summary to first user message
            }
        }

        // 3. Strip thinking blocks (they're captured in summary if we switched)
        stats.stripped_count = strip_thinking_blocks(body);

        // Remove context_management if we stripped anything
        if stats.stripped_count > 0 {
            remove_context_management(body);
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
            "Backend switch detected, summarization will be implemented in Phase 2.5-2.6"
        );
        // TODO: Phase 2.5 - call LLM to summarize last_messages
        // TODO: Store result in pending_summary
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_context() -> TransformContext {
        TransformContext::new("test-backend", "test-request-123")
    }

    fn make_config() -> SummarizeConfig {
        SummarizeConfig::default()
    }

    #[tokio::test]
    async fn name_returns_summarize() {
        let transformer = SummarizeTransformer::new(make_config());
        assert_eq!(transformer.name(), "summarize");
    }

    #[tokio::test]
    async fn saves_messages_on_transform() {
        let transformer = SummarizeTransformer::new(make_config());
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
        let saved = transformer.last_messages.read().await;
        assert!(saved.is_some());
        assert_eq!(saved.as_ref().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn strips_thinking_blocks() {
        let transformer = SummarizeTransformer::new(make_config());
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

        assert!(result.changed);
        assert_eq!(result.stats.stripped_count, 1);

        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }

    #[tokio::test]
    async fn config_accessible() {
        let config = SummarizeConfig {
            model: "test-model".to_string(),
            backend: Some("test-backend".to_string()),
            max_tokens: 100,
            prompt: "Test prompt".to_string(),
        };
        let transformer = SummarizeTransformer::new(config);

        assert_eq!(transformer.config().model, "test-model");
        assert_eq!(transformer.config().max_tokens, 100);
    }
}
