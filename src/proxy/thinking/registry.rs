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
pub struct BlockInfo {
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
    pub blocks: HashMap<u64, BlockInfo>,

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
            let old_backend_name = self.current_backend.clone();
            let old_session = self.current_session;
            self.current_session += 1;
            self.current_backend = new_backend.to_string();
            crate::metrics::app_log("thinking-registry", &format!(
                "Backend switch: {} -> {}, session {} -> {}, cache_size={}",
                if old_session == 0 { "<none>" } else { &old_backend_name },
                new_backend, old_session, self.current_session, self.blocks.len()
            ));
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
                                crate::metrics::app_log("thinking-registry", &format!(
                                    "SSE: registering complete thinking block index={} len={}", index, accumulated.len()
                                ));
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
                crate::metrics::app_log("thinking-registry", &format!(
                    "SSE: registering thinking block without content_block_stop index={} len={}", index, accumulated.len()
                ));
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
                crate::metrics::app_log("thinking-registry", &format!(
                    "Block already registered in session {}, skipping (hash={}, confirmed={})", session_id, hash, existing.confirmed
                ));
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

        crate::metrics::app_log("thinking-registry", &format!(
            "Registered new thinking block hash={} session={} preview={} cache_size={}",
            hash, session_id, truncate(content, 50), self.blocks.len()
        ));
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

        crate::metrics::app_log("thinking-registry", &format!(
            "Processing request: blocks={} cache_size={} session={}",
            request_hashes.len(), self.blocks.len(), self.current_session
        ));

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
            .is_some_and(|msgs| {
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
            crate::metrics::app_log("thinking-registry", &format!(
                "Request processing complete: confirmed={} cleanup(old={} unused={} orphaned={}) filtered={} cache_size={}",
                confirmed_count, cleanup_stats.old_session, cleanup_stats.confirmed_unused,
                cleanup_stats.orphaned, filtered_count, self.blocks.len()
            ));
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
                    crate::metrics::app_log("thinking-registry", &format!(
                        "Confirmed thinking block hash={} session={} age_ms={}",
                        hash, info.session, info.registered_at.elapsed().as_millis()
                    ));
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
                crate::metrics::app_log("thinking-registry", &format!(
                    "Removing block from old session hash={} block_session={} current_session={}",
                    hash, info.session, self.current_session
                ));
                stats.old_session += 1;
                return false;
            }
            true
        });
        stats
    }

    /// Cleanup cache: remove old session blocks and orphaned blocks.
    fn cleanup_cache(&mut self, request_hashes: &HashSet<u64>, now: Instant) -> CleanupStats {
        // If the request carries no thinking blocks at all, it's uninformative
        // about which blocks are still needed (e.g. haiku sub-request where
        // claude-cli strips thinking from history). Only apply Rule 1.
        if request_hashes.is_empty() {
            return self.cleanup_old_sessions();
        }

        let mut stats = CleanupStats::default();
        let threshold = self.orphan_threshold;

        self.blocks.retain(|hash, info| {
            // Rule 1: Old session - always remove
            if info.session != self.current_session {
                crate::metrics::app_log("thinking-registry", &format!(
                    "Removing block from old session hash={} block_session={} current_session={}",
                    hash, info.session, self.current_session
                ));
                stats.old_session += 1;
                return false;
            }

            // Rule 2: Confirmed but not in request - remove
            if info.confirmed && !request_hashes.contains(hash) {
                crate::metrics::app_log("thinking-registry", &format!(
                    "Removing confirmed block no longer in request hash={} session={} age_ms={}",
                    hash, info.session, info.registered_at.elapsed().as_millis()
                ));
                stats.confirmed_unused += 1;
                return false;
            }

            // Rule 3: Unconfirmed, not in request, and old - remove (orphan)
            if !info.confirmed && !request_hashes.contains(hash) {
                let age = now.duration_since(info.registered_at);
                if age > threshold {
                    crate::metrics::app_log("thinking-registry", &format!(
                        "Removing orphaned block hash={} session={} age_ms={} threshold_ms={}",
                        hash, info.session, age.as_millis(), threshold.as_millis()
                    ));
                    stats.orphaned += 1;
                    return false;
                } else {
                    crate::metrics::app_log("thinking-registry", &format!(
                        "Keeping unconfirmed block (within grace period) hash={} session={} age_ms={} threshold_ms={}",
                        hash, info.session, age.as_millis(), threshold.as_millis()
                    ));
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
                    crate::metrics::app_log("thinking-registry", "Removing thinking block: failed to extract content");
                    return false;
                };

                let hash = fast_hash(&thinking);

                // Check if block is in cache (implies valid session)
                if self.blocks.contains_key(&hash) {
                    crate::metrics::app_log("thinking-registry", &format!(
                        "Keeping thinking block in request (found in cache) hash={}", hash
                    ));
                    true
                } else {
                    crate::metrics::app_log("thinking-registry", &format!(
                        "Removing thinking block from request (not in cache) hash={} preview={}",
                        hash, truncate(&thinking, 50)
                    ));
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
        crate::metrics::app_log("thinking-registry", &format!(
            "Cache state: total={} confirmed={} unconfirmed={} current_session_blocks={} old_session_blocks={} session={} backend={}",
            stats.total, stats.confirmed, stats.unconfirmed, stats.current_session,
            stats.old_session, self.current_session, self.current_backend
        ));
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
pub fn fast_hash(content: &str) -> u64 {
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
pub fn safe_truncate(s: &str, max_bytes: usize) -> &str {
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
pub fn safe_suffix(s: &str, max_bytes: usize) -> &str {
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

// Tests in tests/thinking_registry.rs
