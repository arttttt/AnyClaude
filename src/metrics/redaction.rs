use axum::http::HeaderMap;
use serde_json::Value;

const REDACTED: &str = "****";

pub fn redact_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    let mut output = Vec::with_capacity(headers.len());
    for (name, value) in headers.iter() {
        let key = name.as_str().to_string();
        let value_str = value.to_str().unwrap_or("<non-utf8>");
        if is_sensitive_header(name.as_str()) {
            output.push((key, mask_value(value_str)));
        } else {
            output.push((key, value_str.to_string()));
        }
    }
    output
}

pub fn redact_body_preview(bytes: &[u8], content_type: &str, limit: usize) -> Option<String> {
    redact_body(bytes, content_type, Some(limit), false)
}

/// Redact and format body with configurable options.
///
/// - `limit`: Max bytes to include (None = unlimited)
/// - `pretty`: Pretty-print JSON output
pub fn redact_body(
    bytes: &[u8],
    content_type: &str,
    limit: Option<usize>,
    pretty: bool,
) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }

    // Handle SSE streams specially
    if content_type.contains("text/event-stream") {
        return Some(summarize_sse_stream(bytes, pretty));
    }

    // Apply limit if specified
    let data = match limit {
        Some(0) => return None,
        Some(l) if bytes.len() > l => &bytes[..l],
        _ => bytes,
    };

    if content_type.contains("application/json") {
        // For full body, parse entire bytes even if we have a limit on display
        let parse_bytes = if limit.is_none() { bytes } else { data };
        let mut value = match serde_json::from_slice::<Value>(parse_bytes) {
            Ok(val) => val,
            Err(_) => return Some(mask_tokens(&String::from_utf8_lossy(data))),
        };
        redact_json_value(&mut value);

        let result = if pretty {
            serde_json::to_string_pretty(&value).ok()
        } else {
            serde_json::to_string(&value).ok()
        };
        return result;
    }

    Some(mask_tokens(&String::from_utf8_lossy(data)))
}

/// Parse SSE stream and return a structured summary instead of raw events.
fn summarize_sse_stream(bytes: &[u8], pretty: bool) -> String {
    let events = crate::sse::parse_sse_events(bytes);

    let mut event_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut last_message_delta: Option<Value> = None;
    let mut final_text = String::new();
    let total_events = events.len() as u32;
    let mut error_event: Option<Value> = None;

    for event in &events {
        *event_counts.entry(event.event_type.clone()).or_insert(0) += 1;

        match event.event_type.as_str() {
            "content_block_delta" => {
                if let Some(text) = event.data
                    .get("delta")
                    .and_then(|d| d.get("text"))
                    .and_then(|t| t.as_str())
                {
                    final_text.push_str(text);
                }
            }
            "message_delta" => {
                last_message_delta = Some(event.data.clone());
            }
            "error" => {
                error_event = Some(event.data.clone());
            }
            _ => {}
        }
    }

    // Build summary
    let mut summary = serde_json::json!({
        "sse_summary": {
            "total_events": total_events,
            "event_counts": event_counts,
        }
    });

    // Add final text preview (truncated)
    if !final_text.is_empty() {
        let preview = if final_text.len() > 500 {
            format!("{}...[truncated, total {} chars]", &final_text[..500], final_text.len())
        } else {
            final_text
        };
        summary["sse_summary"]["text_preview"] = Value::String(preview);
    }

    // Add usage info from message_delta
    if let Some(delta) = last_message_delta {
        if let Some(usage) = delta.get("usage") {
            summary["sse_summary"]["usage"] = usage.clone();
        }
        if let Some(stop_reason) = delta.get("delta").and_then(|d| d.get("stop_reason")) {
            summary["sse_summary"]["stop_reason"] = stop_reason.clone();
        }
    }

    // Add error if present
    if let Some(err) = error_event {
        summary["sse_summary"]["error"] = err;
    }

    if pretty {
        serde_json::to_string_pretty(&summary).unwrap_or_else(|_| format!("SSE stream: {} events", total_events))
    } else {
        serde_json::to_string(&summary).unwrap_or_else(|_| format!("SSE stream: {} events", total_events))
    }
}

fn redact_json_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if is_sensitive_key(key) {
                    *val = Value::String(mask_value(val.as_str().unwrap_or("")));
                } else {
                    redact_json_value(val);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_json_value(item);
            }
        }
        _ => {}
    }
}

fn is_sensitive_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "proxy-authorization" | "x-api-key" | "cookie" | "set-cookie"
    )
}

fn is_sensitive_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "api_key" | "authorization" | "access_token" | "refresh_token" | "secret" | "password"
    )
}

fn mask_tokens(input: &str) -> String {
    let mut output = input.to_string();

    output = mask_bearer_tokens(&output);
    output = mask_key_value(&output, "api_key");
    output = mask_key_value(&output, "access_token");
    output = mask_key_value(&output, "refresh_token");

    output
}

fn mask_bearer_tokens(input: &str) -> String {
    let marker = "Bearer ";
    if !input.contains(marker) {
        return input.to_string();
    }

    let mut result = String::new();
    let mut rest = input;
    while let Some(pos) = rest.find(marker) {
        let (before, after) = rest.split_at(pos);
        result.push_str(before);
        result.push_str(marker);
        let token_start = marker.len();
        let token = after[token_start..].split_whitespace().next().unwrap_or("");
        result.push_str(&mask_value(token));
        rest = &after[token_start + token.len()..];
    }
    result.push_str(rest);
    result
}

fn mask_key_value(input: &str, key: &str) -> String {
    let pattern = format!("{}=", key);
    if !input.contains(&pattern) {
        return input.to_string();
    }

    let mut result = String::new();
    let mut rest = input;
    while let Some(pos) = rest.find(&pattern) {
        let (before, after) = rest.split_at(pos);
        result.push_str(before);
        result.push_str(&pattern);
        let value = after[pattern.len()..]
            .split(|c: char| c == '&' || c.is_whitespace())
            .next()
            .unwrap_or("");
        result.push_str(&mask_value(value));
        rest = &after[pattern.len() + value.len()..];
    }
    result.push_str(rest);
    result
}

fn mask_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return REDACTED.to_string();
    }

    let last = trimmed.chars().rev().take(4).collect::<String>();
    format!("{}{}", REDACTED, last.chars().rev().collect::<String>())
}
