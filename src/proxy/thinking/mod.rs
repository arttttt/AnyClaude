//! Thinking block transformation system.
//!
//! This module provides a flexible system for handling Claude's thinking blocks
//! when proxying requests to different backends.
//!
//! # Modes
//!
//! - **Strip**: Remove thinking blocks entirely (simplest, most compatible)
//! - **Summarize**: Keep native during work, summarize on backend switch
//! - **Native**: Keep native format, requires handoff on switch (future)
//!
//! # Architecture
//!
//! ```text
//! ThinkingTransformer (trait)
//!         ▲
//!         │
//!    ┌────┴────┬────────────┐
//!    │         │            │
//! Strip   Summarize    Native
//!    │         │            │
//!    └────┬────┴────────────┘
//!         │
//!         ▼
//! TransformerRegistry (creates & stores active transformer)
//! ```

mod context;
mod error;
mod native;
mod registry;
mod sse_parser;
mod strip;
mod summarize;
mod summarizer;
mod traits;

pub use context::{TransformContext, TransformResult, TransformStats};
pub use error::{SummarizeError, TransformError};
pub use native::NativeTransformer;
pub use registry::{CacheStats, ThinkingRegistry};
pub use sse_parser::extract_assistant_text;
pub use strip::{remove_context_management, strip_thinking_blocks, StripTransformer};
pub use summarize::SummarizeTransformer;
pub use summarizer::SummarizerClient;
pub use traits::ThinkingTransformer;

use crate::config::{ThinkingConfig, ThinkingMode};
use crate::metrics::DebugLogger;
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Registry that creates and manages the active thinking transformer.
///
/// Supports hot-swapping transformers when configuration changes.
/// Also manages the ThinkingRegistry for tracking thinking blocks across sessions.
pub struct TransformerRegistry {
    current: RwLock<Arc<dyn ThinkingTransformer>>,
    config: std::sync::RwLock<ThinkingConfig>,
    debug_logger: Option<Arc<DebugLogger>>,
    /// Registry for tracking thinking blocks by session
    thinking_registry: Mutex<ThinkingRegistry>,
}

impl TransformerRegistry {
    /// Create a new registry with the given thinking config.
    pub fn new(config: ThinkingConfig, debug_logger: Option<Arc<DebugLogger>>) -> Self {
        let transformer = Self::create_transformer(&config, debug_logger.clone());
        Self {
            current: RwLock::new(transformer),
            config: std::sync::RwLock::new(config),
            debug_logger,
            thinking_registry: Mutex::new(ThinkingRegistry::new()),
        }
    }

    /// Create a new registry with just a mode (uses default summarize config).
    /// Note: This constructor is for testing without debug logging.
    pub fn with_mode(mode: ThinkingMode) -> Self {
        Self::new(
            ThinkingConfig {
                mode,
                ..Default::default()
            },
            None,
        )
    }

    /// Create a transformer for the given config.
    fn create_transformer(
        config: &ThinkingConfig,
        debug_logger: Option<Arc<DebugLogger>>,
    ) -> Arc<dyn ThinkingTransformer> {
        match config.mode {
            ThinkingMode::Strip => Arc::new(StripTransformer),

            ThinkingMode::Summarize => {
                tracing::info!("Creating SummarizeTransformer");
                Arc::new(SummarizeTransformer::new(config.summarize.clone(), debug_logger))
            }

            ThinkingMode::Native => {
                tracing::info!("Creating NativeTransformer (passthrough, no summarization)");
                Arc::new(NativeTransformer::new())
            }
        }
    }

    /// Get the current transformer.
    ///
    /// Returns a clone of the Arc'd transformer, so the caller owns a reference
    /// and no lock is held after this returns.
    pub async fn get(&self) -> Arc<dyn ThinkingTransformer> {
        self.current.read().await.clone()
    }

    /// Update the thinking mode (hot-swap transformer).
    pub async fn update_mode(&self, mode: ThinkingMode) {
        // Check and update mode with sync lock, then drop it before async work
        let new_config = {
            let mut current_config = self.config.write().expect("config lock poisoned");
            if current_config.mode != mode {
                tracing::info!(
                    old_mode = ?current_config.mode,
                    new_mode = ?mode,
                    "Switching thinking transformer"
                );
                current_config.mode = mode;
                Some(current_config.clone())
            } else {
                None
            }
        }; // sync lock dropped here

        // Now do async work without holding the sync lock
        if let Some(config) = new_config {
            let transformer = Self::create_transformer(&config, self.debug_logger.clone());
            *self.current.write().await = transformer;
        }
    }

    /// Get the current mode.
    pub fn current_mode(&self) -> ThinkingMode {
        self.config.read().expect("config lock poisoned").mode.clone()
    }

    /// Get the current config.
    pub fn current_config(&self) -> ThinkingConfig {
        self.config.read().expect("config lock poisoned").clone()
    }

    /// Trigger backend switch on the current transformer.
    ///
    /// This calls `on_backend_switch` on the underlying transformer,
    /// which for SummarizeTransformer will summarize the session.
    pub async fn on_backend_switch(
        &self,
        from_backend: &str,
        to_backend: &str,
    ) -> Result<(), super::thinking::TransformError> {
        let transformer = self.get().await;
        let mut body = serde_json::json!({});
        transformer
            .on_backend_switch(from_backend, to_backend, &mut body)
            .await
    }

    /// Notify the transformer that a response is complete.
    ///
    /// This parses the SSE response bytes and extracts the assistant's text,
    /// then forwards it to the transformer for potential storage.
    pub async fn on_response_complete(&self, sse_bytes: &[u8]) {
        if let Some(text) = extract_assistant_text(sse_bytes) {
            let transformer = self.get().await;
            transformer.on_response_complete(text).await;
        }
    }

    // ========================================================================
    // Thinking Registry methods
    // ========================================================================

    /// Notify the thinking registry about a backend switch.
    ///
    /// This increments the internal session ID, invalidating thinking blocks
    /// from previous sessions.
    pub fn notify_backend_for_thinking(&self, backend_name: &str) {
        let mut registry = self.thinking_registry.lock();
        registry.on_backend_switch(backend_name);
    }

    /// Register thinking blocks from a response body.
    ///
    /// Extracts thinking blocks and records them with the current session.
    pub fn register_thinking_from_response(&self, response_body: &[u8]) {
        let mut registry = self.thinking_registry.lock();
        registry.register_from_response(response_body);
    }

    /// Register thinking blocks from pre-parsed SSE events.
    ///
    /// Accumulates thinking deltas and registers complete thinking blocks.
    pub fn register_thinking_from_sse_stream(&self, events: &[crate::sse::SseEvent]) {
        let mut registry = self.thinking_registry.lock();
        registry.register_from_sse_stream(events);
    }

    /// Filter thinking blocks in a request body.
    ///
    /// This is the main entry point for request processing. It performs:
    /// 1. Confirm: Mark blocks present in request as confirmed
    /// 2. Cleanup: Remove old/orphaned blocks from cache
    /// 3. Filter: Remove invalid blocks from request body
    ///
    /// Returns the number of blocks removed from the request.
    pub fn filter_thinking_blocks(&self, body: &mut serde_json::Value) -> u32 {
        let mut registry = self.thinking_registry.lock();
        registry.filter_request(body)
    }

    /// Get the current thinking session ID.
    pub fn current_thinking_session(&self) -> u64 {
        let registry = self.thinking_registry.lock();
        registry.current_session()
    }

    /// Get cache statistics for monitoring.
    pub fn thinking_cache_stats(&self) -> CacheStats {
        let registry = self.thinking_registry.lock();
        registry.cache_stats()
    }

    /// Log current thinking cache state (for debugging).
    pub fn log_thinking_cache_state(&self) {
        let registry = self.thinking_registry.lock();
        registry.log_cache_state();
    }
}

impl std::fmt::Debug for TransformerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let config = self.config.read().expect("config lock poisoned");
        f.debug_struct("TransformerRegistry")
            .field("mode", &config.mode)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SummarizeConfig;

    #[tokio::test]
    async fn registry_creates_strip_transformer() {
        let registry = TransformerRegistry::with_mode(ThinkingMode::Strip);
        let transformer = registry.get().await;
        assert_eq!(transformer.name(), "strip");
    }

    #[tokio::test]
    async fn registry_creates_summarize_transformer() {
        let registry = TransformerRegistry::with_mode(ThinkingMode::Summarize);
        let transformer = registry.get().await;
        assert_eq!(transformer.name(), "summarize");
    }

    #[tokio::test]
    async fn registry_hot_swaps_transformer() {
        let registry = TransformerRegistry::with_mode(ThinkingMode::Strip);
        assert_eq!(registry.current_mode(), ThinkingMode::Strip);

        // Hot swap to Summarize
        registry.update_mode(ThinkingMode::Summarize).await;
        assert_eq!(registry.current_mode(), ThinkingMode::Summarize);
        let transformer = registry.get().await;
        assert_eq!(transformer.name(), "summarize");
    }

    #[tokio::test]
    async fn registry_with_full_config() {
        let config = ThinkingConfig {
            mode: ThinkingMode::Summarize,
            summarize: SummarizeConfig {
                base_url: "https://api.example.com".to_string(),
                api_key: Some("test-key".to_string()),
                model: "test-model".to_string(),
                max_tokens: 100,
            },
        };
        let registry = TransformerRegistry::new(config, None);
        let transformer = registry.get().await;
        assert_eq!(transformer.name(), "summarize");
        assert_eq!(registry.current_mode(), ThinkingMode::Summarize);
    }
}
