//! Core trait for thinking block transformers.

use async_trait::async_trait;
use serde_json::Value;

use super::context::{TransformContext, TransformResult};
use super::error::TransformError;

/// Trait for transforming thinking blocks in API requests.
///
/// Each thinking mode (strip, summarize, native) implements this trait.
/// Transformers are called on every request to potentially modify
/// the message history before sending to the upstream API.
#[async_trait]
pub trait ThinkingTransformer: Send + Sync {
    /// Returns the name of this transformer for logging.
    fn name(&self) -> &'static str;

    /// Transform the request body before sending upstream.
    ///
    /// This is called on every API request. The transformer can modify
    /// the `body` in place to transform thinking blocks.
    ///
    /// # Arguments
    /// * `body` - The JSON request body (mutable)
    /// * `context` - Context about the current request
    ///
    /// # Returns
    /// * `Ok(TransformResult)` - Result with stats about what was transformed
    /// * `Err(TransformError)` - If transformation failed
    async fn transform_request(
        &self,
        body: &mut Value,
        context: &TransformContext,
    ) -> Result<TransformResult, TransformError>;

    /// Called when switching backends.
    ///
    /// This allows transformers to perform special handling when the user
    /// switches from one backend to another. For example, the `summarize`
    /// mode uses this to summarize all thinking blocks before the switch.
    ///
    /// Default implementation does nothing.
    ///
    /// # Arguments
    /// * `from_backend` - Name of the current backend
    /// * `to_backend` - Name of the target backend
    /// * `body` - The message history (mutable)
    async fn on_backend_switch(
        &self,
        _from_backend: &str,
        _to_backend: &str,
        _body: &mut Value,
    ) -> Result<(), TransformError> {
        Ok(())
    }

    /// Called when a response is complete with the assistant's text content.
    ///
    /// This allows transformers to capture the assistant's response for
    /// later use (e.g., summarization on backend switch).
    ///
    /// Default implementation does nothing.
    ///
    /// # Arguments
    /// * `response_text` - The assistant's text response
    async fn on_response_complete(&self, _response_text: String) {
        // Default: do nothing
    }

    /// Whether this transformer requires async operations.
    ///
    /// If false, the transformer can be used in sync contexts.
    /// Default is false (most transformers are sync).
    fn is_async(&self) -> bool {
        false
    }
}
