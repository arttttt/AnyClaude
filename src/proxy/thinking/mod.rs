//! Thinking block lifecycle management.
//!
//! Tracks thinking blocks across backend switches. When the active backend
//! changes, old thinking blocks become invalid (signatures don't match).
//! The registry tracks blocks by session and filters invalid ones from requests.
//!
//! # Architecture
//!
//! - **ThinkingRegistry**: Core block tracking (session-based filter + cleanup)
//! - **TransformerRegistry**: Thread-safe wrapper around ThinkingRegistry
//! - **ThinkingSession**: Per-request handle for the thinking lifecycle

mod registry;
pub use registry::{fast_hash, safe_suffix, safe_truncate, BlockInfo, CacheStats, ThinkingRegistry};

use std::sync::Arc;

use parking_lot::Mutex;

use crate::metrics::DebugLogger;

/// Thread-safe wrapper around ThinkingRegistry.
///
/// Shared across all requests via `Arc`. Individual requests obtain
/// a [`ThinkingSession`] via [`begin_request`](TransformerRegistry::begin_request).
pub struct TransformerRegistry {
    /// Registry for tracking thinking blocks by session
    thinking_registry: Mutex<ThinkingRegistry>,
}

impl TransformerRegistry {
    /// Create a new registry.
    pub fn new() -> Self {
        crate::metrics::app_log("thinking", "Creating TransformerRegistry");
        Self {
            thinking_registry: Mutex::new(ThinkingRegistry::new()),
        }
    }

    /// Begin a new request's thinking lifecycle.
    ///
    /// Atomically notifies the registry about the current backend
    /// (incrementing the session if the backend changed) and captures
    /// the session ID. Returns a [`ThinkingSession`] handle that owns
    /// the session ID for filter and registration operations.
    ///
    /// This combines `notify_backend_for_thinking` + `current_thinking_session`
    /// into a single lock acquisition, eliminating the race condition where
    /// another thread could increment the session between the two calls.
    pub fn begin_request(
        self: &Arc<Self>,
        backend: &str,
        debug_logger: Arc<DebugLogger>,
    ) -> ThinkingSession {
        let mut reg = self.thinking_registry.lock();
        reg.on_backend_switch(backend);
        let session_id = reg.current_session();
        ThinkingSession {
            registry: Arc::clone(self),
            session_id,
            debug_logger,
        }
    }

    /// Notify about a backend switch (e.g. from IPC command).
    ///
    /// Increments the thinking session if the backend changed,
    /// invalidating blocks from the previous backend.
    pub fn notify_backend_switch(&self, backend: &str) {
        let mut reg = self.thinking_registry.lock();
        reg.on_backend_switch(backend);
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

/// Per-request handle for the thinking block lifecycle.
///
/// Created by [`TransformerRegistry::begin_request`] in the thinking middleware,
/// placed into request extensions, and consumed by `do_forward()`.
///
/// Holds a captured `session_id` that remains stable for the entire
/// request-response cycle, even if other requests trigger backend switches
/// concurrently.
/// Clone required by `http::Extensions::insert()`.
/// Cheap: all fields are `Arc` or `u64`.
#[derive(Clone)]
pub struct ThinkingSession {
    registry: Arc<TransformerRegistry>,
    session_id: u64,
    debug_logger: Arc<DebugLogger>,
}

impl std::fmt::Debug for ThinkingSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThinkingSession")
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl ThinkingSession {
    /// Filter invalid thinking blocks from a request body.
    ///
    /// Returns the number of blocks removed.
    pub fn filter(&self, body: &mut serde_json::Value) -> u32 {
        let mut reg = self.registry.thinking_registry.lock();
        let cache_size = reg.cache_stats().total;
        let filtered = reg.filter_request(body);
        if filtered > 0 || cache_size > 0 {
            self.debug_logger.log_auxiliary(
                "thinking_filter",
                None,
                None,
                Some(&format!(
                    "Filter: cache={} blocks, removed={} from request",
                    cache_size, filtered,
                )),
                None,
            );
        }
        filtered
    }

    /// Register thinking blocks from a completed SSE stream.
    pub fn register_from_sse(&self, events: &[crate::sse::SseEvent]) {
        let thinking_stats = crate::sse::analyze_thinking_stream(events);
        self.debug_logger.log_auxiliary(
            "sse_callback",
            None,
            None,
            Some(&format!(
                "SSE callback: {} events, thinking: {}",
                events.len(),
                thinking_stats,
            )),
            None,
        );

        let mut reg = self.registry.thinking_registry.lock();
        let before = reg.cache_stats().total;
        reg.register_from_sse_stream(events, self.session_id);
        let after = reg.cache_stats().total;
        drop(reg);
        let registered = after.saturating_sub(before);
        self.debug_logger.log_auxiliary(
            "sse_callback",
            None,
            None,
            Some(&format!(
                "Registered {} new thinking blocks (cache: {} → {})",
                registered, before, after,
            )),
            None,
        );
    }

    /// Register thinking blocks from a non-streaming response body.
    pub fn register_from_response(&self, response_body: &[u8]) {
        if response_body.is_empty() {
            return;
        }
        if serde_json::from_slice::<serde_json::Value>(response_body).is_err() {
            self.debug_logger.log_auxiliary(
                "thinking_registry",
                None,
                None,
                Some(&format!(
                    "Failed to parse response body as JSON ({} bytes), skipping thinking registration",
                    response_body.len(),
                )),
                None,
            );
            return;
        }
        let mut reg = self.registry.thinking_registry.lock();
        reg.register_from_response(response_body, self.session_id);
    }
}
