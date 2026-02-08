//! SSE (Server-Sent Events) parser.
//!
//! Provides a single, robust parser for SSE streams used across the codebase.
//! Handles format variations (e.g. `data:{...}` vs `data: {...}`).

use serde_json::Value;
use std::collections::HashSet;

/// A parsed SSE event.
pub struct SseEvent {
    /// Event type from the `type` field in JSON data.
    pub event_type: String,
    /// Full parsed JSON payload.
    pub data: Value,
}

impl SseEvent {
    /// Returns true if this event is a thinking-related SSE event.
    ///
    /// Matches:
    /// - `content_block_start` with `thinking` or `redacted_thinking` type
    /// - `content_block_delta` with `thinking_delta` or `signature_delta` type
    ///
    /// Note: `content_block_stop` cannot be classified here because it only
    /// carries an `index` field — no block type info. Use `analyze_thinking_stream()`
    /// for full stateful analysis including stop events.
    pub fn is_thinking_event(&self) -> bool {
        match self.event_type.as_str() {
            "content_block_start" => {
                let block_type = self
                    .data
                    .get("content_block")
                    .and_then(|b| b.get("type"))
                    .and_then(|t| t.as_str());
                matches!(block_type, Some("thinking" | "redacted_thinking"))
            }
            "content_block_delta" => {
                let delta_type = self
                    .data
                    .get("delta")
                    .and_then(|d| d.get("type"))
                    .and_then(|t| t.as_str());
                matches!(delta_type, Some("thinking_delta" | "signature_delta"))
            }
            _ => false,
        }
    }
}

/// Count thinking-related SSE events in a byte stream.
pub fn count_thinking_events(bytes: &[u8]) -> usize {
    parse_sse_events(bytes)
        .iter()
        .filter(|e| e.is_thinking_event())
        .count()
}

/// Statistics from full stateful analysis of thinking events in an SSE stream.
#[derive(Debug, Default)]
pub struct ThinkingStreamStats {
    /// Number of `content_block_start` events with type `thinking`.
    pub thinking_blocks: usize,
    /// Number of `content_block_start` events with type `redacted_thinking`.
    pub redacted_blocks: usize,
    /// Number of `content_block_delta` events with type `thinking_delta`.
    pub thinking_deltas: usize,
    /// Number of `content_block_delta` events with type `signature_delta`.
    pub signature_deltas: usize,
    /// Number of `content_block_stop` events for thinking block indices.
    pub thinking_stops: usize,
    /// Whether any non-empty signature data was found (in start or delta).
    pub has_signatures: bool,
}

impl ThinkingStreamStats {
    /// Total number of thinking-related events.
    pub fn total(&self) -> usize {
        self.thinking_blocks
            + self.redacted_blocks
            + self.thinking_deltas
            + self.signature_deltas
            + self.thinking_stops
    }
}

impl std::fmt::Display for ThinkingStreamStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} blocks ({} redacted), {} deltas, {} sig_deltas, {} stops, signatures: {}",
            self.thinking_blocks,
            self.redacted_blocks,
            self.thinking_deltas,
            self.signature_deltas,
            self.thinking_stops,
            if self.has_signatures { "found" } else { "none" },
        )
    }
}

/// Analyze an SSE event stream for thinking-related events with full state tracking.
///
/// Unlike `is_thinking_event()` (stateless, per-event), this tracks block indices
/// to correctly attribute `content_block_stop` events to thinking blocks and
/// detect `signature_delta` events.
pub fn analyze_thinking_stream(events: &[SseEvent]) -> ThinkingStreamStats {
    let mut stats = ThinkingStreamStats::default();
    let mut thinking_indices: HashSet<u64> = HashSet::new();

    for event in events {
        match event.event_type.as_str() {
            "content_block_start" => {
                let block_type = event
                    .data
                    .get("content_block")
                    .and_then(|b| b.get("type"))
                    .and_then(|t| t.as_str());
                let index = event.data.get("index").and_then(|i| i.as_u64());

                match block_type {
                    Some("thinking") => {
                        stats.thinking_blocks += 1;
                        if let Some(idx) = index {
                            thinking_indices.insert(idx);
                        }
                        // GLM-style: signature already present in content_block_start
                        let sig = event
                            .data
                            .get("content_block")
                            .and_then(|b| b.get("signature"))
                            .and_then(|s| s.as_str())
                            .unwrap_or("");
                        if !sig.is_empty() {
                            stats.has_signatures = true;
                        }
                    }
                    Some("redacted_thinking") => {
                        stats.redacted_blocks += 1;
                        if let Some(idx) = index {
                            thinking_indices.insert(idx);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let delta_type = event
                    .data
                    .get("delta")
                    .and_then(|d| d.get("type"))
                    .and_then(|t| t.as_str());

                match delta_type {
                    Some("thinking_delta") => {
                        stats.thinking_deltas += 1;
                    }
                    Some("signature_delta") => {
                        stats.signature_deltas += 1;
                        let sig = event
                            .data
                            .get("delta")
                            .and_then(|d| d.get("signature"))
                            .and_then(|s| s.as_str())
                            .unwrap_or("");
                        if !sig.is_empty() {
                            stats.has_signatures = true;
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(idx) = event.data.get("index").and_then(|i| i.as_u64()) {
                    if thinking_indices.contains(&idx) {
                        stats.thinking_stops += 1;
                    }
                }
            }
            _ => {}
        }
    }

    stats
}

/// Parse SSE stream bytes into structured events.
///
/// Handles:
/// - `data: {...}` (standard, with space)
/// - `data:{...}` (compact, no space — used by some providers)
/// - `[DONE]` markers and non-JSON lines are skipped
/// - Non-data lines (comments, event:, id:, empty) are skipped
pub fn parse_sse_events(bytes: &[u8]) -> Vec<SseEvent> {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .filter_map(parse_sse_line)
        .collect()
}

/// Extract a JSON event from a line of text.
///
/// Tries two strategies:
/// 1. Parse the line as JSON directly (handles raw JSON, non-SSE responses)
/// 2. Strip SSE `data:` prefix and parse the remainder
fn parse_sse_line(line: &str) -> Option<SseEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let json: Value = serde_json::from_str(line)
        .ok()
        .or_else(|| {
            let data = line.strip_prefix("data:")?.trim_start();
            serde_json::from_str(data).ok()
        })?;

    let event_type = json.get("type")?.as_str()?.to_string();
    Some(SseEvent { event_type, data: json })
}
