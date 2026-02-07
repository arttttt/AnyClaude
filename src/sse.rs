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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_format() {
        let sse = b"data: {\"type\": \"message_start\", \"message\": {}}\n";
        let events = parse_sse_events(sse);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "message_start");
    }

    #[test]
    fn parses_compact_format() {
        let sse = b"data:{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n";
        let events = parse_sse_events(sse);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "content_block_start");
    }

    #[test]
    fn skips_done_marker() {
        let sse = b"data: {\"type\": \"message_stop\"}\ndata: [DONE]\n";
        let events = parse_sse_events(sse);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "message_stop");
    }

    #[test]
    fn skips_non_data_lines() {
        let sse = b"event: message\ndata: {\"type\": \"ping\"}\n\n: comment\n";
        let events = parse_sse_events(sse);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "ping");
    }

    #[test]
    fn handles_mixed_formats() {
        let sse = b"data: {\"type\": \"a\"}\ndata:{\"type\": \"b\"}\ndata:  {\"type\": \"c\"}\n";
        let events = parse_sse_events(sse);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_type, "a");
        assert_eq!(events[1].event_type, "b");
        // "  {..." — strip_prefix(' ') removes one space, JSON parser handles the rest
        assert_eq!(events[2].event_type, "c");
    }

    #[test]
    fn empty_stream() {
        let events = parse_sse_events(b"");
        assert!(events.is_empty());
    }

    #[test]
    fn is_thinking_event_content_block_start() {
        let sse = b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n";
        let events = parse_sse_events(sse);
        assert!(events[0].is_thinking_event());
    }

    #[test]
    fn is_thinking_event_redacted() {
        let sse = b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"abc\"}}\n";
        let events = parse_sse_events(sse);
        assert!(events[0].is_thinking_event());
    }

    #[test]
    fn is_thinking_event_delta() {
        let sse = b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hello\"}}\n";
        let events = parse_sse_events(sse);
        assert!(events[0].is_thinking_event());
    }

    #[test]
    fn is_not_thinking_event_text_delta() {
        let sse = b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"thinking about thinking_compat\"}}\n";
        let events = parse_sse_events(sse);
        assert!(!events[0].is_thinking_event());
    }

    #[test]
    fn is_not_thinking_event_text_block() {
        let sse = b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n";
        let events = parse_sse_events(sse);
        assert!(!events[0].is_thinking_event());
    }

    #[test]
    fn count_thinking_events_mixed_stream() {
        let sse = b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
                     data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hi\"}}\n\
                     data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"thinking about thinking\"}}\n\
                     data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
                     data: {\"type\":\"message_start\",\"message\":{}}\n";
        assert_eq!(count_thinking_events(sse), 2);
    }

    // ========================================================================
    // signature_delta tests
    // ========================================================================

    #[test]
    fn is_thinking_event_signature_delta() {
        let sse = b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"EqQBCgIYA...\"}}\n";
        let events = parse_sse_events(sse);
        assert!(events[0].is_thinking_event());
    }

    #[test]
    fn is_not_thinking_event_input_json_delta() {
        let sse = b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n";
        let events = parse_sse_events(sse);
        assert!(!events[0].is_thinking_event());
    }

    #[test]
    fn count_includes_signature_delta() {
        let sse = b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
                     data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hi\"}}\n\
                     data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig123\"}}\n\
                     data: {\"type\":\"content_block_stop\",\"index\":0}\n";
        // is_thinking_event: start + thinking_delta + signature_delta = 3
        // content_block_stop is NOT counted (stateless, no block type info)
        assert_eq!(count_thinking_events(sse), 3);
    }

    // ========================================================================
    // analyze_thinking_stream tests
    // ========================================================================

    #[test]
    fn analyze_anthropic_style_stream() {
        // Anthropic: empty signature in start, signature_delta before stop
        let sse = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me think\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\" about this\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"EqQBCgIYA...\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Answer\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":1}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.thinking_blocks, 1);
        assert_eq!(stats.redacted_blocks, 0);
        assert_eq!(stats.thinking_deltas, 2);
        assert_eq!(stats.signature_deltas, 1);
        assert_eq!(stats.thinking_stops, 1); // only index 0, not index 1
        assert!(stats.has_signatures);
        assert_eq!(stats.total(), 5); // 1 start + 2 deltas + 1 sig + 1 stop
    }

    #[test]
    fn analyze_glm_style_stream() {
        // GLM: non-empty signature directly in content_block_start
        let sse = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"8aa60582f6c340b4ab362b4b\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hi\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.thinking_blocks, 1);
        assert_eq!(stats.thinking_deltas, 1);
        assert_eq!(stats.signature_deltas, 0); // GLM doesn't use signature_delta
        assert_eq!(stats.thinking_stops, 1);
        assert!(stats.has_signatures); // found in content_block_start
    }

    #[test]
    fn analyze_kimi_style_stream() {
        // Kimi: empty signature in start, no signature_delta
        let sse = b"\
data:{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\
data:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hi\"}}\n\
data:{\"type\":\"content_block_stop\",\"index\":0}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.thinking_blocks, 1);
        assert_eq!(stats.thinking_deltas, 1);
        assert_eq!(stats.signature_deltas, 0);
        assert_eq!(stats.thinking_stops, 1);
        assert!(!stats.has_signatures); // empty signature everywhere
    }

    #[test]
    fn analyze_no_thinking_stream() {
        let sse = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.total(), 0);
        assert!(!stats.has_signatures);
    }

    #[test]
    fn analyze_multiple_thinking_blocks() {
        let sse = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"A\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig1\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"enc\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":1}\n\
data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":2}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.thinking_blocks, 1);
        assert_eq!(stats.redacted_blocks, 1);
        assert_eq!(stats.thinking_deltas, 1);
        assert_eq!(stats.signature_deltas, 1);
        assert_eq!(stats.thinking_stops, 2); // index 0 and 1, not 2
        assert!(stats.has_signatures);
    }

    #[test]
    fn analyze_display_format() {
        let stats = ThinkingStreamStats {
            thinking_blocks: 1,
            redacted_blocks: 0,
            thinking_deltas: 5,
            signature_deltas: 1,
            thinking_stops: 1,
            has_signatures: true,
        };
        assert_eq!(
            stats.to_string(),
            "1 blocks (0 redacted), 5 deltas, 1 sig_deltas, 1 stops, signatures: found"
        );
    }

    #[test]
    fn analyze_display_format_no_signatures() {
        let stats = ThinkingStreamStats {
            thinking_blocks: 2,
            redacted_blocks: 1,
            thinking_deltas: 10,
            signature_deltas: 0,
            thinking_stops: 3,
            has_signatures: false,
        };
        assert_eq!(
            stats.to_string(),
            "2 blocks (1 redacted), 10 deltas, 0 sig_deltas, 3 stops, signatures: none"
        );
    }

    // ========================================================================
    // Edge case / corner case tests
    // ========================================================================

    #[test]
    fn analyze_empty_stream() {
        let events = parse_sse_events(b"");
        let stats = analyze_thinking_stream(&events);
        assert_eq!(stats.total(), 0);
        assert!(!stats.has_signatures);
    }

    #[test]
    fn analyze_truncated_stream_no_stop() {
        // Stream ends mid-block (no content_block_stop)
        let sse = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"truncated\"}}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.thinking_blocks, 1);
        assert_eq!(stats.thinking_deltas, 1);
        assert_eq!(stats.thinking_stops, 0); // never got stop
        assert_eq!(stats.signature_deltas, 0);
        assert!(!stats.has_signatures);
    }

    #[test]
    fn analyze_stop_without_matching_start() {
        // content_block_stop for an index never seen as thinking
        let sse = b"\
data: {\"type\":\"content_block_stop\",\"index\":99}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.thinking_stops, 0); // index 99 wasn't a thinking block
        assert_eq!(stats.total(), 0);
    }

    #[test]
    fn analyze_signature_delta_with_empty_signature() {
        // signature_delta arrives but with empty signature value
        let sse = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.signature_deltas, 1); // event counted
        assert!(!stats.has_signatures); // but signature is empty
    }

    #[test]
    fn analyze_signature_delta_without_signature_field() {
        // Malformed signature_delta: missing the "signature" field entirely
        let sse = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.signature_deltas, 1); // event type matches
        assert!(!stats.has_signatures); // no actual signature data
    }

    #[test]
    fn analyze_interleaved_thinking_and_text_blocks() {
        // Thinking at index 0, text at index 1, thinking at index 2
        let sse = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"A\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"B\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"C\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig0\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig2\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
data: {\"type\":\"content_block_stop\",\"index\":1}\n\
data: {\"type\":\"content_block_stop\",\"index\":2}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.thinking_blocks, 2); // indices 0 and 2
        assert_eq!(stats.thinking_deltas, 2); // A and C
        assert_eq!(stats.signature_deltas, 2); // sig0 and sig2
        assert_eq!(stats.thinking_stops, 2); // stops for 0 and 2
        assert!(stats.has_signatures);
        assert_eq!(stats.total(), 8); // 2 starts + 2 deltas + 2 sigs + 2 stops
    }

    #[test]
    fn analyze_content_block_start_without_index() {
        // Malformed: no index field
        let sse = b"\
data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.thinking_blocks, 1); // block counted
        assert_eq!(stats.thinking_stops, 0); // stop index 0 not in thinking_indices (no index was registered)
    }

    #[test]
    fn analyze_only_redacted_thinking() {
        let sse = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"encrypted-abc\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.thinking_blocks, 0);
        assert_eq!(stats.redacted_blocks, 1);
        assert_eq!(stats.thinking_deltas, 0);
        assert_eq!(stats.signature_deltas, 0);
        assert_eq!(stats.thinking_stops, 1);
        assert!(!stats.has_signatures);
    }

    #[test]
    fn analyze_only_non_thinking_events() {
        // message_start, message_delta, message_stop — no content blocks at all
        let sse = b"\
data: {\"type\":\"message_start\",\"message\":{}}\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\
data: {\"type\":\"message_stop\"}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.total(), 0);
        assert!(!stats.has_signatures);
    }

    #[test]
    fn analyze_duplicate_stop_for_same_index() {
        // Two stop events for the same thinking index (shouldn't happen, but handle gracefully)
        let sse = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.thinking_blocks, 1);
        assert_eq!(stats.thinking_stops, 2); // both counted
    }

    #[test]
    fn is_thinking_event_content_block_stop_is_false() {
        // Verify content_block_stop is NOT classified as thinking event in stateless check
        let sse = b"data: {\"type\":\"content_block_stop\",\"index\":0}\n";
        let events = parse_sse_events(sse);
        assert!(!events[0].is_thinking_event());
    }

    #[test]
    fn is_thinking_event_message_start_is_false() {
        let sse = b"data: {\"type\":\"message_start\",\"message\":{}}\n";
        let events = parse_sse_events(sse);
        assert!(!events[0].is_thinking_event());
    }

    #[test]
    fn analyze_glm_signature_in_start_plus_anthropic_delta() {
        // Hypothetical hybrid: signature in start AND signature_delta
        let sse = b"\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"start-sig\"}}\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"delta-sig\"}}\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n";

        let events = parse_sse_events(sse);
        let stats = analyze_thinking_stream(&events);

        assert_eq!(stats.thinking_blocks, 1);
        assert_eq!(stats.signature_deltas, 1);
        assert_eq!(stats.thinking_stops, 1);
        assert!(stats.has_signatures); // found in both places
    }
}
