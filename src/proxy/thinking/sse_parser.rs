//! SSE response parser for extracting assistant text from Claude API streaming responses.
//!
//! This module parses Server-Sent Events (SSE) format used by Claude API
//! to extract the text content of the assistant's response.

use serde::Deserialize;

/// Parse SSE response bytes and extract the assistant's text content.
///
/// Claude API sends SSE events like:
/// ```text
/// data: {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "Hello"}}
/// data: {"type": "message_stop"}
/// ```
///
/// This function accumulates all text_delta content and returns the full response.
pub fn extract_assistant_text(sse_bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(sse_bytes);
    let mut result = String::new();

    for line in text.lines() {
        // SSE data lines start with "data: "
        if let Some(json_str) = line.strip_prefix("data: ") {
            // Skip [DONE] marker
            if json_str.trim() == "[DONE]" {
                continue;
            }

            // Try to parse as a delta event
            if let Ok(event) = serde_json::from_str::<SseEvent>(json_str) {
                if let Some(delta) = event.delta {
                    if delta.delta_type == "text_delta" {
                        if let Some(text) = delta.text {
                            result.push_str(&text);
                        }
                    }
                }
            }
        }
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// SSE event from Claude API.
#[derive(Debug, Deserialize)]
struct SseEvent {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    event_type: String,
    delta: Option<Delta>,
}

/// Delta content in SSE event.
#[derive(Debug, Deserialize)]
struct Delta {
    #[serde(rename = "type")]
    delta_type: String,
    text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_text_from_sse_events() {
        let sse = b"data: {\"type\": \"content_block_delta\", \"delta\": {\"type\": \"text_delta\", \"text\": \"Hello \"}}\n\
                    data: {\"type\": \"content_block_delta\", \"delta\": {\"type\": \"text_delta\", \"text\": \"world!\"}}\n\
                    data: {\"type\": \"message_stop\"}\n";

        let result = extract_assistant_text(sse);
        assert_eq!(result, Some("Hello world!".to_string()));
    }

    #[test]
    fn handles_empty_response() {
        let sse = b"data: {\"type\": \"message_start\"}\n\
                    data: {\"type\": \"message_stop\"}\n";

        let result = extract_assistant_text(sse);
        assert_eq!(result, None);
    }

    #[test]
    fn handles_thinking_blocks() {
        // Thinking blocks have different type, should be ignored
        let sse = b"data: {\"type\": \"content_block_delta\", \"delta\": {\"type\": \"thinking_delta\", \"thinking\": \"Let me think...\"}}\n\
                    data: {\"type\": \"content_block_delta\", \"delta\": {\"type\": \"text_delta\", \"text\": \"Here's the answer\"}}\n\
                    data: {\"type\": \"message_stop\"}\n";

        let result = extract_assistant_text(sse);
        assert_eq!(result, Some("Here's the answer".to_string()));
    }

    #[test]
    fn handles_done_marker() {
        let sse = b"data: {\"type\": \"content_block_delta\", \"delta\": {\"type\": \"text_delta\", \"text\": \"Test\"}}\n\
                    data: [DONE]\n";

        let result = extract_assistant_text(sse);
        assert_eq!(result, Some("Test".to_string()));
    }
}
