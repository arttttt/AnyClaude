//! SSE (Server-Sent Events) parser.
//!
//! Provides a single, robust parser for SSE streams used across the codebase.
//! Handles format variations (e.g. `data:{...}` vs `data: {...}`).

use serde_json::Value;

/// A parsed SSE event.
pub struct SseEvent {
    /// Event type from the `type` field in JSON data.
    pub event_type: String,
    /// Full parsed JSON payload.
    pub data: Value,
}

impl SseEvent {
    /// Returns true if this event is a thinking-related SSE event
    /// (content_block_start with thinking/redacted_thinking, or thinking_delta).
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
                matches!(delta_type, Some("thinking_delta"))
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
}
