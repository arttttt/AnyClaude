//! Thinking block registry - tracks which thinking blocks belong to which session.
//!
//! When switching backends, old thinking blocks become invalid (signatures don't match).
//! This registry tracks thinking blocks by hashing their content and associating them
//! with an internal session ID that increments on each backend switch.
//!
//! # Lifecycle of a thinking block
//!
//! 1. **Registration**: Block registered from response (confirmed=false)
//! 2. **Confirmation**: Block seen in subsequent request (confirmed=true)
//! 3. **Cleanup**: Block removed when no longer needed
//!
//! # Cleanup rules
//!
//! A block is removed if:
//! - `session ≠ current_session` (old session, always remove)
//! - `session = current AND confirmed AND ∉ request` (no longer used)
//! - `session = current AND !confirmed AND ∉ request AND age > threshold` (orphaned)

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, Instant};

/// Default threshold for orphan cleanup (unconfirmed blocks older than this are removed).
const DEFAULT_ORPHAN_THRESHOLD: Duration = Duration::from_secs(300); // 5 minutes

/// Information about a registered thinking block.
#[derive(Debug, Clone)]
struct BlockInfo {
    /// Session ID when this block was registered.
    session: u64,
    /// Whether this block has been seen in a request (confirmed as used by CC).
    confirmed: bool,
    /// When this block was registered.
    registered_at: Instant,
}

/// Registry for tracking thinking blocks across backend switches.
///
/// Each thinking block is identified by a hash of its content (prefix + length).
/// When a backend switch occurs, the session ID increments, invalidating
/// all previous thinking blocks.
#[derive(Debug)]
pub struct ThinkingRegistry {
    /// Current session ID (increments on each backend switch).
    current_session: u64,

    /// Current backend name.
    current_backend: String,

    /// Map of content_hash → block info.
    blocks: HashMap<u64, BlockInfo>,

    /// Threshold for orphan cleanup.
    orphan_threshold: Duration,
}

impl Default for ThinkingRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ThinkingRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            current_session: 0,
            current_backend: String::new(),
            blocks: HashMap::new(),
            orphan_threshold: DEFAULT_ORPHAN_THRESHOLD,
        }
    }

    /// Create a new registry with a custom orphan threshold.
    pub fn with_orphan_threshold(threshold: Duration) -> Self {
        Self {
            current_session: 0,
            current_backend: String::new(),
            blocks: HashMap::new(),
            orphan_threshold: threshold,
        }
    }

    /// Called when the backend changes. Increments the session ID.
    ///
    /// This invalidates all thinking blocks from previous sessions.
    pub fn on_backend_switch(&mut self, new_backend: &str) {
        if self.current_backend != new_backend {
            let old_session = self.current_session;
            self.current_session += 1;
            self.current_backend = new_backend.to_string();
            tracing::info!(
                old_backend = %if old_session == 0 { "<none>" } else { &self.current_backend },
                new_backend = %new_backend,
                old_session = old_session,
                new_session = self.current_session,
                cache_size = self.blocks.len(),
                "Backend switch: incremented thinking session"
            );
        }
    }

    /// Register thinking blocks from a response.
    ///
    /// Extracts thinking blocks from the response and records their hashes
    /// with the given session ID. The session_id should be captured at request
    /// time to avoid races with concurrent backend switches.
    pub fn register_from_response(&mut self, response_body: &[u8], session_id: u64) {
        let Ok(json) = serde_json::from_slice::<Value>(response_body) else {
            return;
        };

        if let Some(content) = json.get("content").and_then(|c| c.as_array()) {
            for item in content {
                if let Some(thinking) = extract_thinking_content(item) {
                    self.register_block(&thinking, session_id);
                }
            }
        }
    }

    /// Register thinking blocks from a complete SSE stream.
    ///
    /// Parses the full SSE byte stream, accumulates `thinking_delta` events
    /// per block index, and registers complete thinking blocks.
    ///
    /// SSE event sequence for thinking blocks:
    /// 1. `content_block_start` with `{"type":"thinking","thinking":""}` → start accumulator
    /// 2. `content_block_delta` with `{"type":"thinking_delta","thinking":"chunk"}` → append
    /// 3. `content_block_stop` → register complete block
    ///
    /// Redacted thinking blocks are complete in `content_block_start` and registered immediately.
    ///
    /// The session_id should be captured at request time to avoid races with
    /// concurrent backend switches.
    pub fn register_from_sse_stream(&mut self, events: &[crate::sse::SseEvent], session_id: u64) {
        let mut accumulators: HashMap<u64, String> = HashMap::new();

        for event in events {
            match event.event_type.as_str() {
                "content_block_start" => {
                    let Some(block) = event.data.get("content_block") else {
                        continue;
                    };
                    let block_type = block.get("type").and_then(|t| t.as_str());
                    match block_type {
                        Some("thinking") => {
                            if let Some(index) = event.data.get("index").and_then(|i| i.as_u64()) {
                                let initial = block
                                    .get("thinking")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("");
                                accumulators.insert(index, initial.to_string());
                            }
                        }
                        Some("redacted_thinking") => {
                            if let Some(data) = block.get("data").and_then(|d| d.as_str()) {
                                self.register_block(data, session_id);
                            }
                        }
                        _ => {}
                    }
                }
                "content_block_delta" => {
                    if let Some(delta) = event.data.get("delta") {
                        if delta.get("type").and_then(|t| t.as_str()) == Some("thinking_delta") {
                            if let Some(index) = event.data.get("index").and_then(|i| i.as_u64()) {
                                if let Some(thinking) =
                                    delta.get("thinking").and_then(|t| t.as_str())
                                {
                                    if let Some(acc) = accumulators.get_mut(&index) {
                                        acc.push_str(thinking);
                                    }
                                }
                            }
                        }
                    }
                }
                "content_block_stop" => {
                    if let Some(index) = event.data.get("index").and_then(|i| i.as_u64()) {
                        if let Some(accumulated) = accumulators.remove(&index) {
                            if !accumulated.is_empty() {
                                tracing::debug!(
                                    index = index,
                                    content_len = accumulated.len(),
                                    "SSE: registering complete thinking block"
                                );
                                self.register_block(&accumulated, session_id);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Register any remaining accumulators (stream may have been truncated)
        for (index, accumulated) in accumulators {
            if !accumulated.is_empty() {
                tracing::warn!(
                    index = index,
                    content_len = accumulated.len(),
                    "SSE: registering thinking block without content_block_stop"
                );
                self.register_block(&accumulated, session_id);
            }
        }
    }

    /// Register a single thinking block under the given session ID.
    fn register_block(&mut self, content: &str, session_id: u64) {
        let hash = fast_hash(content);
        let now = Instant::now();

        // Check if already registered
        if let Some(existing) = self.blocks.get(&hash) {
            if existing.session == session_id {
                tracing::trace!(
                    hash = hash,
                    session = session_id,
                    already_confirmed = existing.confirmed,
                    "Block already registered in current session, skipping"
                );
                return;
            }
        }

        self.blocks.insert(
            hash,
            BlockInfo {
                session: session_id,
                confirmed: false,
                registered_at: now,
            },
        );

        tracing::debug!(
            hash = hash,
            session = session_id,
            content_preview = %truncate(content, 50),
            cache_size = self.blocks.len(),
            "Registered new thinking block"
        );
    }

    /// Process a request: confirm blocks, cleanup cache, filter request body.
    ///
    /// This is the main entry point for request processing. It performs:
    /// 1. **Confirm**: Mark blocks present in request as confirmed
    /// 2. **Cleanup**: Remove old/orphaned blocks from cache
    /// 3. **Filter**: Remove invalid blocks from request body
    ///
    /// Returns the number of blocks removed from the request.
    pub fn filter_request(&mut self, body: &mut Value) -> u32 {
        let now = Instant::now();

        // Step 1: Extract all thinking block hashes from request
        let request_hashes = self.extract_request_hashes(body);

        tracing::debug!(
            request_blocks = request_hashes.len(),
            cache_size = self.blocks.len(),
            current_session = self.current_session,
            "Processing request"
        );

        // Step 2: Confirm blocks that are in the request
        let confirmed_count = self.confirm_blocks(&request_hashes);

        // Step 3: Cleanup cache (remove old session blocks and orphans).
        // Only run cache eviction (rules 2-3) when the request carries
        // conversation history (has assistant messages). Requests without
        // assistant messages (count_tokens, first user turn, etc.) have
        // no history to compare against and would incorrectly evict blocks.
        let has_history = body
            .get("messages")
            .and_then(|v| v.as_array())
            .map_or(false, |msgs| {
                msgs.iter().any(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
            });
        let cleanup_stats = if has_history {
            self.cleanup_cache(&request_hashes, now)
        } else {
            // Still remove old-session blocks (Rule 1 only)
            self.cleanup_old_sessions()
        };

        // Step 4: Filter request body (remove blocks not in cache)
        let filtered_count = self.filter_request_body(body);

        // Log summary
        if confirmed_count > 0 || cleanup_stats.total_removed() > 0 || filtered_count > 0 {
            tracing::info!(
                confirmed = confirmed_count,
                cleanup_old_session = cleanup_stats.old_session,
                cleanup_confirmed_unused = cleanup_stats.confirmed_unused,
                cleanup_orphaned = cleanup_stats.orphaned,
                filtered_from_request = filtered_count,
                cache_size_after = self.blocks.len(),
                "Request processing complete"
            );
        }

        filtered_count
    }

    /// Extract all thinking block hashes from a request body.
    fn extract_request_hashes(&self, body: &Value) -> HashSet<u64> {
        let mut hashes = HashSet::new();

        let Some(messages) = body.get("messages").and_then(|v| v.as_array()) else {
            return hashes;
        };

        for message in messages {
            let Some(content) = message.get("content").and_then(|v| v.as_array()) else {
                continue;
            };

            for item in content {
                if let Some(thinking) = extract_thinking_content(item) {
                    hashes.insert(fast_hash(&thinking));
                }
            }
        }

        hashes
    }

    /// Confirm blocks that are present in the request.
    /// Returns the number of blocks newly confirmed.
    fn confirm_blocks(&mut self, request_hashes: &HashSet<u64>) -> u32 {
        let mut confirmed_count = 0u32;

        for hash in request_hashes {
            if let Some(info) = self.blocks.get_mut(hash) {
                if info.session == self.current_session && !info.confirmed {
                    info.confirmed = true;
                    confirmed_count += 1;
                    tracing::debug!(
                        hash = hash,
                        session = info.session,
                        age_ms = info.registered_at.elapsed().as_millis() as u64,
                        "Confirmed thinking block"
                    );
                }
            }
        }

        confirmed_count
    }

    /// Remove only old-session blocks (Rule 1). Used when the request has no
    /// conversation history and full cleanup would incorrectly evict blocks.
    fn cleanup_old_sessions(&mut self) -> CleanupStats {
        let mut stats = CleanupStats::default();
        self.blocks.retain(|hash, info| {
            if info.session != self.current_session {
                tracing::debug!(
                    hash = hash,
                    block_session = info.session,
                    current_session = self.current_session,
                    "Removing block from old session"
                );
                stats.old_session += 1;
                return false;
            }
            true
        });
        stats
    }

    /// Cleanup cache: remove old session blocks and orphaned blocks.
    fn cleanup_cache(&mut self, request_hashes: &HashSet<u64>, now: Instant) -> CleanupStats {
        let mut stats = CleanupStats::default();
        let threshold = self.orphan_threshold;

        self.blocks.retain(|hash, info| {
            // Rule 1: Old session - always remove
            if info.session != self.current_session {
                tracing::debug!(
                    hash = hash,
                    block_session = info.session,
                    current_session = self.current_session,
                    "Removing block from old session"
                );
                stats.old_session += 1;
                return false;
            }

            // Rule 2: Confirmed but not in request - remove
            if info.confirmed && !request_hashes.contains(hash) {
                tracing::debug!(
                    hash = hash,
                    session = info.session,
                    age_ms = info.registered_at.elapsed().as_millis() as u64,
                    "Removing confirmed block no longer in request"
                );
                stats.confirmed_unused += 1;
                return false;
            }

            // Rule 3: Unconfirmed, not in request, and old - remove (orphan)
            if !info.confirmed && !request_hashes.contains(hash) {
                let age = now.duration_since(info.registered_at);
                if age > threshold {
                    tracing::debug!(
                        hash = hash,
                        session = info.session,
                        age_ms = age.as_millis() as u64,
                        threshold_ms = threshold.as_millis() as u64,
                        "Removing orphaned block (unconfirmed and expired)"
                    );
                    stats.orphaned += 1;
                    return false;
                } else {
                    tracing::trace!(
                        hash = hash,
                        session = info.session,
                        age_ms = age.as_millis() as u64,
                        threshold_ms = threshold.as_millis() as u64,
                        "Keeping unconfirmed block (within grace period)"
                    );
                }
            }

            true
        });

        stats
    }

    /// Filter request body: remove thinking blocks not in cache.
    fn filter_request_body(&self, body: &mut Value) -> u32 {
        let Some(messages) = body.get_mut("messages").and_then(|v| v.as_array_mut()) else {
            return 0;
        };

        let mut removed_count = 0u32;

        for message in messages.iter_mut() {
            let Some(content) = message.get_mut("content").and_then(|v| v.as_array_mut()) else {
                continue;
            };

            let before_len = content.len();

            content.retain(|item| {
                // Keep non-thinking blocks
                let item_type = item.get("type").and_then(|t| t.as_str());
                if !matches!(item_type, Some("thinking") | Some("redacted_thinking")) {
                    return true;
                }

                // Extract content and compute hash
                let Some(thinking) = extract_thinking_content(item) else {
                    tracing::debug!("Removing thinking block: failed to extract content");
                    return false;
                };

                let hash = fast_hash(&thinking);

                // Check if block is in cache (implies valid session)
                if self.blocks.contains_key(&hash) {
                    tracing::trace!(
                        hash = hash,
                        "Keeping thinking block in request (found in cache)"
                    );
                    true
                } else {
                    tracing::debug!(
                        hash = hash,
                        content_preview = %truncate(&thinking, 50),
                        "Removing thinking block from request (not in cache)"
                    );
                    false
                }
            });

            removed_count += (before_len - content.len()) as u32;
        }

        removed_count
    }

    /// Get the current session ID.
    pub fn current_session(&self) -> u64 {
        self.current_session
    }

    /// Get the current backend name.
    pub fn current_backend(&self) -> &str {
        &self.current_backend
    }

    /// Get the number of registered blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Get cache statistics for monitoring.
    pub fn cache_stats(&self) -> CacheStats {
        let mut confirmed = 0;
        let mut unconfirmed = 0;
        let mut current_session = 0;
        let mut old_session = 0;

        for info in self.blocks.values() {
            if info.confirmed {
                confirmed += 1;
            } else {
                unconfirmed += 1;
            }
            if info.session == self.current_session {
                current_session += 1;
            } else {
                old_session += 1;
            }
        }

        CacheStats {
            total: self.blocks.len(),
            confirmed,
            unconfirmed,
            current_session,
            old_session,
        }
    }

    /// Log current cache state (for debugging).
    pub fn log_cache_state(&self) {
        let stats = self.cache_stats();
        tracing::info!(
            total = stats.total,
            confirmed = stats.confirmed,
            unconfirmed = stats.unconfirmed,
            current_session_blocks = stats.current_session,
            old_session_blocks = stats.old_session,
            session_id = self.current_session,
            backend = %self.current_backend,
            "Thinking block cache state"
        );
    }
}

/// Statistics from cache cleanup.
#[derive(Debug, Default)]
struct CleanupStats {
    old_session: u32,
    confirmed_unused: u32,
    orphaned: u32,
}

impl CleanupStats {
    fn total_removed(&self) -> u32 {
        self.old_session + self.confirmed_unused + self.orphaned
    }
}

/// Cache statistics for monitoring.
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total: usize,
    pub confirmed: usize,
    pub unconfirmed: usize,
    pub current_session: usize,
    pub old_session: usize,
}

/// Extract thinking content from a JSON value.
fn extract_thinking_content(item: &Value) -> Option<String> {
    let item_type = item.get("type").and_then(|t| t.as_str())?;

    match item_type {
        "thinking" => item
            .get("thinking")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string()),
        "redacted_thinking" => item
            .get("data")
            .and_then(|d| d.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

/// Fast hash using prefix + suffix + length for reliability.
///
/// Hashes:
/// - First ~256 bytes (UTF-8 safe)
/// - Last ~256 bytes (UTF-8 safe)
/// - Total length
///
/// This provides good uniqueness while being fast for large content.
/// Two blocks with same prefix but different endings will have different hashes.
fn fast_hash(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();

    // Hash prefix (first ~256 bytes, adjusted to char boundary)
    let prefix = safe_truncate(content, 256);
    prefix.hash(&mut hasher);

    // Hash suffix (last ~256 bytes, adjusted to char boundary)
    let suffix = safe_suffix(content, 256);
    suffix.hash(&mut hasher);

    // Hash the total length
    content.len().hash(&mut hasher);

    hasher.finish()
}

/// Safely truncate a string from the start at a char boundary.
fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Safely get suffix of a string at a char boundary.
fn safe_suffix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let start = s.len() - max_bytes;
    // Find the first valid char boundary at or after start
    let mut begin = start;
    while begin < s.len() && !s.is_char_boundary(begin) {
        begin += 1;
    }
    &s[begin..]
}

/// Truncate a string for logging.
fn truncate(s: &str, max_len: usize) -> String {
    let truncated = safe_truncate(s, max_len);
    if truncated.len() < s.len() {
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ========================================================================
    // Helper functions for tests
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

        let events = crate::sse::parse_sse_events(sse_stream);
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

        let events = crate::sse::parse_sse_events(sse_stream);
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

        let events = crate::sse::parse_sse_events(sse_stream);
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

        let events = crate::sse::parse_sse_events(sse_stream);
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

        // Register block
        let response = make_response_with_thinking(&["Thought A"]);
        registry.register_from_response(&response, registry.current_session());

        // Request with history but without the block - should remove as orphan (threshold=0)
        let mut request = json!({"messages": [
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi"}
        ]});
        registry.filter_request(&mut request);

        assert_eq!(registry.block_count(), 0);
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

        // Turn 1: Response with thinking
        let response1 = make_response_with_thinking(&["Analyzing the problem..."]);
        registry.register_from_response(&response1, registry.current_session());
        assert_eq!(registry.block_count(), 1);

        // Turn 2: Request includes previous thinking
        let mut request2 = make_request_with_thinking(&["Analyzing the problem..."]);
        let removed = registry.filter_request(&mut request2);
        assert_eq!(removed, 0);
        assert_eq!(registry.cache_stats().confirmed, 1);

        // Turn 2: Response with new thinking
        let response2 = make_response_with_thinking(&["Let me elaborate..."]);
        registry.register_from_response(&response2, registry.current_session());
        assert_eq!(registry.block_count(), 2);

        // Turn 3: Request includes both thoughts
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

        // Build up several thoughts
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

        // Context truncation: only keep last 2 thoughts
        let mut request = make_request_with_thinking(&["Thought 4", "Thought 5"]);
        registry.filter_request(&mut request);

        // Thoughts 1-3 should be removed
        assert_eq!(registry.block_count(), 2);
    }

    #[test]
    fn test_full_flow_backend_switch() {
        let mut registry = ThinkingRegistry::new();

        // Session 1: anthropic
        registry.on_backend_switch("anthropic");
        let response1 = make_response_with_thinking(&["Anthropic thought"]);
        registry.register_from_response(&response1, registry.current_session());

        let mut request1 = make_request_with_thinking(&["Anthropic thought"]);
        registry.filter_request(&mut request1);
        assert_eq!(registry.cache_stats().confirmed, 1);

        // Switch to GLM
        registry.on_backend_switch("glm");

        // Request still has old thought (CC hasn't updated)
        let mut request2 = make_request_with_thinking(&["Anthropic thought"]);
        let removed = registry.filter_request(&mut request2);
        assert_eq!(removed, 1); // Old thought removed
        assert_eq!(registry.block_count(), 0);

        // GLM response with new thought
        let response2 = make_response_with_thinking(&["GLM thought"]);
        registry.register_from_response(&response2, registry.current_session());

        // Request with new thought
        let mut request3 = make_request_with_thinking(&["GLM thought"]);
        let removed = registry.filter_request(&mut request3);
        assert_eq!(removed, 0);
        assert_eq!(registry.block_count(), 1);
    }

    #[test]
    fn test_full_flow_rapid_backend_switches() {
        let mut registry = ThinkingRegistry::new();

        // Rapid switches without any blocks
        registry.on_backend_switch("a");
        registry.on_backend_switch("b");
        registry.on_backend_switch("c");
        registry.on_backend_switch("a"); // Back to a

        assert_eq!(registry.current_session(), 4);

        // Register and use a block
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

        // Messages with string content (not array)
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

        // Thinking block without content
        let mut request = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking"},  // Missing "thinking" field
                    {"type": "text", "text": "Hello"}
                ]
            }]
        });
        let removed = registry.filter_request(&mut request);
        assert_eq!(removed, 1); // Malformed block removed
    }

    #[test]
    fn test_negative_unknown_block_type() {
        let mut registry = ThinkingRegistry::new();
        registry.on_backend_switch("anthropic");

        // Unknown block type should be kept
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
        let events = crate::sse::parse_sse_events(b"data: not json\n");
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
        // Known limitation: if first 256 and last 256 bytes are same,
        // and length is same, hashes will collide.
        // This is acceptable for our use case - thinking blocks rarely
        // have identical starts AND ends with different middles.
        let prefix = "START_".repeat(50); // ~300 bytes
        let suffix = "_END".repeat(70); // ~280 bytes

        let content1 = format!("{}MIDDLE_A{}", prefix, suffix);
        let content2 = format!("{}MIDDLE_B{}", prefix, suffix);

        let hash1 = fast_hash(&content1);
        let hash2 = fast_hash(&content2);

        // These WILL collide - documenting expected behavior
        assert_eq!(hash1, hash2, "Known limitation: same prefix+suffix+length = same hash");
    }

    #[test]
    fn test_fast_hash_same_prefix_different_suffix() {
        // Same first 256 bytes, different endings
        let prefix = "X".repeat(300);
        let content1 = format!("{}ENDING_AAA", prefix);
        let content2 = format!("{}ENDING_BBB", prefix);

        let hash1 = fast_hash(&content1);
        let hash2 = fast_hash(&content2);

        // Suffix hashing should catch the difference
        assert_ne!(hash1, hash2);
    }

    // ========================================================================
    // Cache stats tests
    // ========================================================================

    #[test]
    fn test_cache_stats() {
        let mut registry = ThinkingRegistry::new();
        registry.on_backend_switch("anthropic");

        // Register some blocks
        let response = make_response_with_thinking(&["A", "B", "C"]);
        registry.register_from_response(&response, registry.current_session());

        let stats = registry.cache_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.unconfirmed, 3);
        assert_eq!(stats.confirmed, 0);
        assert_eq!(stats.current_session, 3);
        assert_eq!(stats.old_session, 0);

        // Confirm some
        let mut request = make_request_with_thinking(&["A", "B"]);
        registry.filter_request(&mut request);

        let stats = registry.cache_stats();
        assert_eq!(stats.confirmed, 2);
        assert_eq!(stats.unconfirmed, 1);
    }
}
