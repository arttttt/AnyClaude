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
    if limit == 0 || bytes.is_empty() {
        return None;
    }

    let preview = if bytes.len() > limit {
        &bytes[..limit]
    } else {
        bytes
    };
    if content_type.contains("application/json") {
        let mut value = match serde_json::from_slice::<Value>(preview) {
            Ok(val) => val,
            Err(_) => return Some(mask_tokens(&String::from_utf8_lossy(preview))),
        };
        redact_json_value(&mut value);
        return serde_json::to_string(&value).ok();
    }

    Some(mask_tokens(&String::from_utf8_lossy(preview)))
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
    match name.to_ascii_lowercase().as_str() {
        "authorization" | "proxy-authorization" | "x-api-key" | "cookie" | "set-cookie" => true,
        _ => false,
    }
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
