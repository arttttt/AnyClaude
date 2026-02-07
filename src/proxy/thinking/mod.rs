//! Thinking block transformation system.
//!
//! This module provides the thinking block handling infrastructure for proxying
//! requests to different backends.
//!
//! # Architecture
//!
//! - **NativeTransformer**: Passthrough transformer (no-op)
//! - **ThinkingRegistry**: Cross-provider block tracking, filters invalid blocks per session
//! - **TransformerRegistry**: Manages transformer + ThinkingRegistry

mod context;
mod error;
mod native;
mod registry;
#[cfg(test)]
mod request_structure_tests;

pub use context::{TransformContext, TransformResult};
pub use error::TransformError;
pub use native::NativeTransformer;
pub use registry::{CacheStats, ThinkingRegistry};

use parking_lot::Mutex;

/// Registry that manages the thinking transformer and ThinkingRegistry.
pub struct TransformerRegistry {
    transformer: NativeTransformer,
    /// Registry for tracking thinking blocks by session
    thinking_registry: Mutex<ThinkingRegistry>,
}

impl TransformerRegistry {
    /// Create a new registry.
    pub fn new() -> Self {
        tracing::info!("Creating NativeTransformer (passthrough)");
        Self {
            transformer: NativeTransformer::new(),
            thinking_registry: Mutex::new(ThinkingRegistry::new()),
        }
    }

    /// Get a reference to the transformer.
    pub fn transformer(&self) -> &NativeTransformer {
        &self.transformer
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
    /// Extracts thinking blocks and records them with the given session ID.
    /// The session_id should be captured at request time to avoid races
    /// with concurrent backend switches.
    pub fn register_thinking_from_response(&self, response_body: &[u8], session_id: u64) {
        let mut registry = self.thinking_registry.lock();
        registry.register_from_response(response_body, session_id);
    }

    /// Register thinking blocks from pre-parsed SSE events.
    ///
    /// Accumulates thinking deltas and registers complete thinking blocks.
    /// The session_id should be captured at request time to avoid races
    /// with concurrent backend switches.
    pub fn register_thinking_from_sse_stream(&self, events: &[crate::sse::SseEvent], session_id: u64) {
        let mut registry = self.thinking_registry.lock();
        registry.register_from_sse_stream(events, session_id);
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
}

impl Default for TransformerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TransformerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformerRegistry").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_creates_native_transformer() {
        let registry = TransformerRegistry::new();
        assert_eq!(registry.transformer().name(), "native");
    }
}
