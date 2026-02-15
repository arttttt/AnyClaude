//! Tests for reverse model mapping in proxy responses.
//!
//! Unit tests for `make_reverse_model_rewriter()` and `reverse_model_in_response()`,
//! plus integration tests that verify the full proxy pipeline rewrites model names
//! in both SSE streaming and non-streaming JSON responses.

mod common;

use axum::body::Bytes;
use anyclaude::proxy::model_rewrite::{
    make_reverse_model_rewriter, reverse_model_in_response, ModelMapping,
};

fn mapping(backend: &str, original: &str) -> ModelMapping {
    ModelMapping {
        backend: backend.to_string(),
        original: original.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Unit tests: make_reverse_model_rewriter (SSE streaming)
// ---------------------------------------------------------------------------

#[test]
fn rewriter_rewrites_model_in_message_start() {
    let chunk = Bytes::from(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\",\"model\":\"glm-5\",\"role\":\"assistant\",\"content\":[]}}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk);
    let text = String::from_utf8_lossy(&result);
    assert!(text.contains("claude-opus-4-6"), "should contain original model");
    assert!(!text.contains("\"glm-5\""), "should not contain backend model");
}

#[test]
fn rewriter_skips_non_message_start_chunk() {
    let chunk = Bytes::from(
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk.clone());
    assert_eq!(result.as_ref(), chunk.as_ref(), "non-message_start chunk should be unchanged");
}

#[test]
fn rewriter_handles_model_mismatch() {
    // Backend returns a different model name than expected — m5: should log
    let chunk = Bytes::from(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"unexpected-model\",\"role\":\"assistant\"}}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk.clone());
    let text = String::from_utf8_lossy(&result);
    assert!(text.contains("unexpected-model"), "mismatched model should be unchanged");
    assert!(!text.contains("claude-opus-4-6"), "should not inject original model when no match");
}

#[test]
fn rewriter_is_stateful_noop_after_first_rewrite() {
    let chunk1 = Bytes::from(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"glm-5\",\"role\":\"assistant\"}}\n\n"
    );
    let chunk2 = Bytes::from(
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hello\"}}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));

    let result1 = rewriter(chunk1);
    assert!(String::from_utf8_lossy(&result1).contains("claude-opus-4-6"));

    let result2 = rewriter(chunk2.clone());
    assert_eq!(result2.as_ref(), chunk2.as_ref());
}

#[test]
fn rewriter_noop_after_message_start_even_if_another_appears() {
    let chunk1 = Bytes::from(
        "data: {\"type\":\"message_start\",\"message\":{\"model\":\"glm-5\"}}\n\n"
    );
    let chunk2 = Bytes::from(
        "data: {\"type\":\"message_start\",\"message\":{\"model\":\"glm-5\"}}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));

    let result1 = rewriter(chunk1);
    assert!(String::from_utf8_lossy(&result1).contains("claude-opus-4-6"));

    let result2 = rewriter(chunk2);
    let text2 = String::from_utf8_lossy(&result2);
    assert!(text2.contains("\"glm-5\""), "second message_start should not be rewritten");
}

#[test]
fn rewriter_handles_empty_chunk() {
    let chunk = Bytes::from("");
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk.clone());
    assert_eq!(result.as_ref(), chunk.as_ref());
}

#[test]
fn rewriter_handles_ping_event() {
    let chunk = Bytes::from("event: ping\ndata: {\"type\":\"ping\"}\n\n");
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk.clone());
    assert_eq!(result.as_ref(), chunk.as_ref());
}

#[test]
fn rewriter_handles_message_start_with_ping_in_same_chunk() {
    let chunk = Bytes::from(
        "event: ping\ndata: {\"type\":\"ping\"}\n\nevent: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"glm-5\",\"role\":\"assistant\"}}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk);
    let text = String::from_utf8_lossy(&result);
    assert!(text.contains("claude-opus-4-6"), "model should be rewritten");
    assert!(text.contains("ping"), "ping event should still be present");
}

#[test]
fn rewriter_handles_compact_data_format() {
    let chunk = Bytes::from(
        "data:{\"type\":\"message_start\",\"message\":{\"model\":\"glm-5\",\"role\":\"assistant\"}}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk);
    let text = String::from_utf8_lossy(&result);
    assert!(text.contains("claude-opus-4-6"), "compact format should also be rewritten");
}

#[test]
fn rewriter_handles_model_with_version_suffix() {
    let chunk = Bytes::from(
        "data: {\"type\":\"message_start\",\"message\":{\"model\":\"k2.5-chat\",\"role\":\"assistant\"}}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("k2.5-chat", "claude-sonnet-4-5-20250929"));
    let result = rewriter(chunk);
    let text = String::from_utf8_lossy(&result);
    assert!(text.contains("claude-sonnet-4-5-20250929"));
    assert!(!text.contains("k2.5-chat"));
}

#[test]
fn rewriter_preserves_other_message_fields() {
    let chunk = Bytes::from(
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_abc\",\"model\":\"glm-5\",\"role\":\"assistant\",\"stop_reason\":null,\"usage\":{\"input_tokens\":100}}}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk);
    let text = String::from_utf8_lossy(&result);
    assert!(text.contains("msg_abc"), "id should be preserved");
    assert!(text.contains("assistant"), "role should be preserved");
    assert!(text.contains("input_tokens"), "usage should be preserved");
    assert!(text.contains("claude-opus-4-6"));
}

#[test]
fn rewriter_handles_no_message_field_in_message_start() {
    let chunk = Bytes::from(
        "data: {\"type\":\"message_start\"}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk);
    let text = String::from_utf8_lossy(&result);
    assert!(text.contains("message_start"));
}

#[test]
fn rewriter_handles_message_without_model_field() {
    let chunk = Bytes::from(
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\",\"role\":\"assistant\"}}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk);
    let text = String::from_utf8_lossy(&result);
    assert!(!text.contains("claude-opus-4-6"));
    assert!(!text.contains("glm-5"));
}

// S7: Negative tests

#[test]
fn rewriter_handles_non_utf8_bytes() {
    // Invalid UTF-8 sequence — should not panic, just pass through
    let chunk = Bytes::from(vec![0xff, 0xfe, 0x00, 0x01]);
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk.clone());
    assert_eq!(result.as_ref(), chunk.as_ref());
}

#[test]
fn rewriter_handles_unicode_model_name() {
    // Model name with unicode characters
    let chunk = Bytes::from(
        "data: {\"type\":\"message_start\",\"message\":{\"model\":\"模型-v1\",\"role\":\"assistant\"}}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("模型-v1", "claude-opus-4-6"));
    let result = rewriter(chunk);
    let text = String::from_utf8_lossy(&result);
    assert!(text.contains("claude-opus-4-6"));
}

#[test]
fn rewriter_handles_malformed_sse_data_line() {
    // data: line with invalid JSON — should not panic
    let chunk = Bytes::from(
        "data: not-json-at-all {\"message_start\"}\n\n"
    );
    let mut rewriter = make_reverse_model_rewriter(mapping("glm-5", "claude-opus-4-6"));
    let result = rewriter(chunk.clone());
    // "message_start" appears in bytes but JSON parse fails — line passes through
    assert_eq!(result.as_ref(), chunk.as_ref());
}

// ---------------------------------------------------------------------------
// Unit tests: reverse_model_in_response (non-streaming JSON)
// ---------------------------------------------------------------------------

#[test]
fn response_rewrites_model_in_json() {
    let body = Bytes::from(
        r#"{"id":"msg_01","type":"message","role":"assistant","model":"glm-5","content":[{"type":"text","text":"hello"}]}"#,
    );
    let result = reverse_model_in_response(&body, &mapping("glm-5", "claude-opus-4-6"));
    let text = String::from_utf8_lossy(&result);
    assert!(text.contains("claude-opus-4-6"));
    assert!(!text.contains("\"glm-5\""));
}

#[test]
fn response_handles_model_mismatch() {
    let body = Bytes::from(
        r#"{"model":"unexpected-model","content":[]}"#,
    );
    let result = reverse_model_in_response(&body, &mapping("glm-5", "claude-opus-4-6"));
    assert_eq!(result.as_ref(), body.as_ref(), "mismatched model should be unchanged");
}

#[test]
fn response_handles_invalid_json() {
    let body = Bytes::from("not json at all");
    let result = reverse_model_in_response(&body, &mapping("glm-5", "claude-opus-4-6"));
    assert_eq!(result.as_ref(), body.as_ref(), "invalid JSON should be unchanged");
}

#[test]
fn response_handles_no_model_field() {
    let body = Bytes::from(r#"{"id":"msg_01","content":[]}"#);
    let result = reverse_model_in_response(&body, &mapping("glm-5", "claude-opus-4-6"));
    assert_eq!(result.as_ref(), body.as_ref());
}

#[test]
fn response_handles_empty_body() {
    let body = Bytes::from("");
    let result = reverse_model_in_response(&body, &mapping("glm-5", "claude-opus-4-6"));
    assert_eq!(result.as_ref(), body.as_ref());
}

#[test]
fn response_preserves_all_other_fields() {
    let body = Bytes::from(
        r#"{"id":"msg_01","type":"message","role":"assistant","model":"k2.5","content":[{"type":"text","text":"world"}],"stop_reason":"end_turn","usage":{"input_tokens":50,"output_tokens":10}}"#,
    );
    let result = reverse_model_in_response(&body, &mapping("k2.5", "claude-sonnet-4-5-20250929"));
    let json: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(json["model"], "claude-sonnet-4-5-20250929");
    assert_eq!(json["id"], "msg_01");
    assert_eq!(json["role"], "assistant");
    assert_eq!(json["stop_reason"], "end_turn");
    assert_eq!(json["usage"]["input_tokens"], 50);
}

#[test]
fn response_handles_error_json() {
    let body = Bytes::from(
        r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
    );
    let result = reverse_model_in_response(&body, &mapping("glm-5", "claude-opus-4-6"));
    assert_eq!(result.as_ref(), body.as_ref(), "error responses have no model field");
}

// S7: negative test for non-JSON body
#[test]
fn response_handles_binary_body() {
    let body = Bytes::from(vec![0x00, 0x01, 0x02, 0x03]);
    let result = reverse_model_in_response(&body, &mapping("glm-5", "claude-opus-4-6"));
    assert_eq!(result.as_ref(), body.as_ref());
}

// ---------------------------------------------------------------------------
// Integration tests: full proxy pipeline with model mapping
// ---------------------------------------------------------------------------

use anyclaude::config::{
    Backend, Config, ConfigStore, DebugLoggingConfig, Defaults, ProxyConfig, TerminalConfig,
};
use anyclaude::metrics::DebugLogger;
use anyclaude::proxy::ProxyServer;
use common::mock_backend::{MockBackend, MockResponse};
use reqwest::Client;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn test_config(backend: Backend, bind_addr: &str) -> Config {
    Config {
        defaults: Defaults {
            active: backend.name.clone(),
            timeout_seconds: 5,
            connect_timeout_seconds: 2,
            idle_timeout_seconds: 30,
            pool_idle_timeout_seconds: 30,
            pool_max_idle_per_host: 2,
            max_retries: 1,
            retry_backoff_base_ms: 10,
        },
        proxy: ProxyConfig {
            bind_addr: bind_addr.to_string(),
            base_url: format!("http://{}", bind_addr),
        },
        terminal: TerminalConfig::default(),
        debug_logging: DebugLoggingConfig::default(),
        claude_settings: HashMap::new(),
        backends: vec![backend],
        agent_teams: None,
    }
}

fn create_backend_with_model_map(
    name: &str,
    base_url: &str,
    model_opus: Option<&str>,
    model_sonnet: Option<&str>,
    model_haiku: Option<&str>,
) -> Backend {
    Backend {
        name: name.to_string(),
        display_name: name.to_uppercase(),
        base_url: base_url.to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: model_opus.map(String::from),
        model_sonnet: model_sonnet.map(String::from),
        model_haiku: model_haiku.map(String::from),
    }
}

fn create_passthrough_backend(name: &str, base_url: &str) -> Backend {
    Backend {
        name: name.to_string(),
        display_name: name.to_uppercase(),
        base_url: base_url.to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    }
}

async fn start_proxy(config: Config) -> (SocketAddr, String, tokio::task::JoinHandle<()>) {
    let config_store = ConfigStore::new(config, PathBuf::from("/tmp/test-reverse-map.toml"));
    let debug_logger = Arc::new(DebugLogger::new(Default::default()));
    let mut server = ProxyServer::new(config_store.clone(), debug_logger).unwrap();
    let (proxy_addr, _base_url) = server.try_bind(&config_store).await.unwrap();
    let handle = tokio::spawn(async move {
        let _ = server.run().await;
    });
    // m6: use wait_for_server instead of sleep
    common::wait_for_server(proxy_addr, Duration::from_secs(5)).await;
    (proxy_addr, format!("http://{}", proxy_addr), handle)
}

// C1 + C2: SSE with Content-Length and forward-mapping verification
#[tokio::test]
async fn integration_sse_reverse_maps_model_in_message_start() {
    let mock = MockBackend::start().await;

    mock.enqueue_response(MockResponse::sse(&[
        r#"{"type":"message_start","message":{"id":"msg_01","model":"glm-5","role":"assistant","content":[]}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        r#"{"type":"message_stop"}"#,
    ])).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let backend = create_backend_with_model_map("test", &mock.base_url(), Some("glm-5"), None, None);
    let config = test_config(backend, &bind_addr);
    let (_addr, proxy_url, _handle) = start_proxy(config).await;

    let client = Client::new();
    let resp = client
        .post(format!("{}/v1/messages", proxy_url))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","stream":true,"max_tokens":1024,"messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    // C1: Content-Length should be stripped when body is rewritten
    assert!(
        resp.headers().get("content-length").is_none(),
        "Content-Length should be stripped when body is rewritten"
    );

    let body = resp.text().await.unwrap();

    assert!(
        body.contains("claude-opus-4-6"),
        "response should contain original model name, got: {}",
        body
    );
    assert!(
        !body.contains("\"glm-5\""),
        "response should not contain backend model name, got: {}",
        body
    );
    assert!(body.contains("Hello"));
    assert!(body.contains("message_stop"));

    // C2: Verify forward mapping was applied to the backend request
    let requests = mock.captured_requests().await;
    assert!(!requests.is_empty(), "mock should have captured a request");
    let req_body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(req_body["model"], "glm-5", "forward mapping should rewrite model to backend value");
}

// C1 + C2: JSON with Content-Length and forward-mapping verification
#[tokio::test]
async fn integration_json_reverse_maps_model() {
    let mock = MockBackend::start().await;

    mock.enqueue_response(MockResponse::json(
        r#"{"id":"msg_01","type":"message","role":"assistant","model":"glm-5","content":[{"type":"text","text":"Hello"}],"stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#,
    )).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let backend = create_backend_with_model_map("test", &mock.base_url(), Some("glm-5"), None, None);
    let config = test_config(backend, &bind_addr);
    let (_addr, proxy_url, _handle) = start_proxy(config).await;

    let client = Client::new();
    let resp = client
        .post(format!("{}/v1/messages", proxy_url))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","stream":false,"max_tokens":1024,"messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    // C1: Stale upstream Content-Length is stripped; hyper may set a correct one
    // from the known-length body. Verify it's either absent or matches the actual body.
    let content_length = resp.headers().get("content-length")
        .map(|v| v.to_str().unwrap().parse::<usize>().unwrap());

    let body = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    if let Some(cl) = content_length {
        assert_eq!(cl, body.len(), "Content-Length must match actual body after rewrite");
    }

    assert_eq!(json["model"], "claude-opus-4-6", "model should be reverse-mapped");
    assert_eq!(json["content"][0]["text"], "Hello", "content should be preserved");

    // C2: Verify forward mapping
    let requests = mock.captured_requests().await;
    let req_body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(req_body["model"], "glm-5", "forward mapping should rewrite model to backend value");
}

#[tokio::test]
async fn integration_no_mapping_passes_through() {
    let mock = MockBackend::start().await;

    mock.enqueue_response(MockResponse::sse(&[
        r#"{"type":"message_start","message":{"model":"claude-opus-4-6","role":"assistant"}}"#,
        r#"{"type":"message_stop"}"#,
    ])).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let backend = create_passthrough_backend("test", &mock.base_url());
    let config = test_config(backend, &bind_addr);
    let (_addr, proxy_url, _handle) = start_proxy(config).await;

    let client = Client::new();
    let resp = client
        .post(format!("{}/v1/messages", proxy_url))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","stream":true,"max_tokens":1024,"messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("claude-opus-4-6"), "without mapping, model passes through as-is");
}

#[tokio::test]
async fn integration_sse_model_mismatch_passes_through() {
    let mock = MockBackend::start().await;

    mock.enqueue_response(MockResponse::sse(&[
        r#"{"type":"message_start","message":{"model":"glm-5-turbo","role":"assistant"}}"#,
        r#"{"type":"message_stop"}"#,
    ])).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let backend = create_backend_with_model_map("test", &mock.base_url(), Some("glm-5"), None, None);
    let config = test_config(backend, &bind_addr);
    let (_addr, proxy_url, _handle) = start_proxy(config).await;

    let client = Client::new();
    let resp = client
        .post(format!("{}/v1/messages", proxy_url))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","stream":true,"max_tokens":1024,"messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("glm-5-turbo"),
        "mismatched model should pass through unchanged"
    );
    assert!(
        !body.contains("claude-opus-4-6"),
        "should not inject original model on mismatch"
    );
}

// M5: Sonnet model family integration test
#[tokio::test]
async fn integration_sse_reverse_maps_sonnet_model() {
    let mock = MockBackend::start().await;

    mock.enqueue_response(MockResponse::sse(&[
        r#"{"type":"message_start","message":{"model":"k2.5-chat","role":"assistant"}}"#,
        r#"{"type":"message_stop"}"#,
    ])).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let backend = create_backend_with_model_map("test", &mock.base_url(), None, Some("k2.5-chat"), None);
    let config = test_config(backend, &bind_addr);
    let (_addr, proxy_url, _handle) = start_proxy(config).await;

    let client = Client::new();
    let resp = client
        .post(format!("{}/v1/messages", proxy_url))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-sonnet-4-5-20250929","stream":true,"max_tokens":1024,"messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("claude-sonnet-4-5-20250929"),
        "sonnet model should be reverse-mapped, got: {}",
        body
    );
    assert!(
        !body.contains("k2.5-chat"),
        "backend model should not appear in response"
    );
}

// M5: Haiku model family integration test
#[tokio::test]
async fn integration_json_reverse_maps_haiku_model() {
    let mock = MockBackend::start().await;

    mock.enqueue_response(MockResponse::json(
        r#"{"id":"msg_01","type":"message","role":"assistant","model":"lite-model","content":[{"type":"text","text":"ok"}]}"#,
    )).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let backend = create_backend_with_model_map("test", &mock.base_url(), None, None, Some("lite-model"));
    let config = test_config(backend, &bind_addr);
    let (_addr, proxy_url, _handle) = start_proxy(config).await;

    let client = Client::new();
    let resp = client
        .post(format!("{}/v1/messages", proxy_url))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-haiku-4-5-20251001","stream":false,"max_tokens":1024,"messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["model"], "claude-haiku-4-5-20251001", "haiku model should be reverse-mapped");
}

// M6: Concurrent request isolation test
#[tokio::test]
async fn integration_concurrent_requests_have_independent_rewriters() {
    let mock = MockBackend::start().await;

    // Enqueue two SSE responses, each with the correct backend model
    mock.enqueue_response(MockResponse::sse(&[
        r#"{"type":"message_start","message":{"model":"glm-5","role":"assistant"}}"#,
        r#"{"type":"message_stop"}"#,
    ])).await;
    mock.enqueue_response(MockResponse::sse(&[
        r#"{"type":"message_start","message":{"model":"glm-5","role":"assistant"}}"#,
        r#"{"type":"message_stop"}"#,
    ])).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let backend = create_backend_with_model_map("test", &mock.base_url(), Some("glm-5"), None, None);
    let config = test_config(backend, &bind_addr);
    let (_addr, proxy_url, _handle) = start_proxy(config).await;

    let client = Client::new();
    let url = format!("{}/v1/messages", proxy_url);
    let body = r#"{"model":"claude-opus-4-6","stream":true,"max_tokens":1024,"messages":[{"role":"user","content":"hi"}]}"#;

    // Send two concurrent requests
    let (resp1, resp2) = tokio::join!(
        client.post(&url).header("content-type", "application/json").body(body).send(),
        client.post(&url).header("content-type", "application/json").body(body).send(),
    );

    let body1 = resp1.unwrap().text().await.unwrap();
    let body2 = resp2.unwrap().text().await.unwrap();

    // Both should have independent rewriters and correct reverse mapping
    assert!(body1.contains("claude-opus-4-6"), "request 1 should have reverse-mapped model");
    assert!(body2.contains("claude-opus-4-6"), "request 2 should have reverse-mapped model");
}

// M7: Error response with model field integration test
#[tokio::test]
async fn integration_error_response_with_model_field_is_rewritten() {
    let mock = MockBackend::start().await;

    // Backend returns 400 error that includes a model field (some backends do this)
    mock.enqueue_response(MockResponse {
        status: 400,
        headers: vec![("content-type".to_string(), "application/json".to_string())],
        body: r#"{"type":"error","model":"glm-5","error":{"type":"invalid_request_error","message":"bad request"}}"#.into(),
        delay_ms: 0,
    }).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let backend = create_backend_with_model_map("test", &mock.base_url(), Some("glm-5"), None, None);
    let config = test_config(backend, &bind_addr);
    let (_addr, proxy_url, _handle) = start_proxy(config).await;

    let client = Client::new();
    let resp = client
        .post(format!("{}/v1/messages", proxy_url))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","stream":false,"max_tokens":1024,"messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    let body = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    // Model should still be reverse-mapped even in error responses
    assert_eq!(json["model"], "claude-opus-4-6", "error response model should be reverse-mapped");
    assert_eq!(json["error"]["message"], "bad request", "error content should be preserved");
}
