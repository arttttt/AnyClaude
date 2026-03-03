//! Integration tests for the 7-stage pipeline.
//!
//! Tests full pipeline execution with various scenarios:
//! - Main pipeline (standard requests)
//! - Teammate pipeline (subagent requests)
//! - Subagent routing via marker models

mod common;

use std::sync::Arc;
use std::time::SystemTime;

use axum::body::Body;
use axum::http::{header::CONTENT_TYPE, Method, Request};

use anyclaude::backend::{BackendState, SubagentBackend};
use anyclaude::config::{Backend, Config, DebugLogDestination, DebugLogFormat, DebugLogLevel, DebugLoggingConfig, Defaults};
use anyclaude::metrics::{DebugLogger, ObservabilityHub, RequestRecord, RequestSpan};
use anyclaude::proxy::pipeline::{self, PipelineContext, PipelineConfig};
use anyclaude::proxy::pool::PoolConfig;
use anyclaude::proxy::thinking::TransformerRegistry;
use anyclaude::proxy::timeout::TimeoutConfig;

use common::mock_backend::{MockBackend, MockResponse};

// =============================================================================
// Integration Test Helpers
// =============================================================================

fn create_integration_config(mock_base_url: &str) -> Config {
    Config {
        defaults: Defaults {
            active: "mock".to_string(),
            timeout_seconds: 5,
            connect_timeout_seconds: 2,
            idle_timeout_seconds: 60,
            pool_idle_timeout_seconds: 90,
            pool_max_idle_per_host: 8,
            max_retries: 1, // Low for faster tests
            retry_backoff_base_ms: 10,
        },
        backends: vec![
            Backend {
                name: "mock".to_string(),
                display_name: "Mock Backend".to_string(),
                base_url: mock_base_url.to_string(),
                auth_type_str: "passthrough".to_string(),
                api_key: None,
                pricing: None,
                thinking_compat: None,
                thinking_budget_tokens: None,
                model_opus: None,
                model_sonnet: Some("mock-sonnet".to_string()),
                model_haiku: Some("mock-haiku".to_string()),
            },
        ],
        ..Default::default()
    }
}

fn create_pipeline_context() -> PipelineContext {
    let record = RequestRecord {
        id: "integration-test-id".to_string(),
        started_at: SystemTime::now(),
        first_byte_at: None,
        completed_at: None,
        latency_ms: None,
        ttfb_ms: None,
        backend: String::new(),
        status: None,
        timed_out: false,
        request_bytes: 0,
        response_bytes: 0,
        request_analysis: None,
        response_analysis: None,
        routing_decision: None,
        request_meta: None,
        response_meta: None,
    };
    let span = RequestSpan::new(record);
    let debug_config = DebugLoggingConfig {
        level: DebugLogLevel::Off,
        format: DebugLogFormat::Console,
        destination: DebugLogDestination::Stderr,
        file_path: "/tmp/test.log".to_string(),
        body_preview_bytes: 1024,
        header_preview: false,
        full_body: false,
        pretty_print: false,
        rotation: Default::default(),
    };
    let debug_logger = Arc::new(DebugLogger::new(debug_config));
    let observability = ObservabilityHub::new(1000);

    PipelineContext::new(span, observability, debug_logger)
}

fn create_pipeline_config(backend_state: BackendState) -> PipelineConfig {
    let subagent_backend = SubagentBackend::new(None);
    let transformer_registry = Arc::new(TransformerRegistry::new());
    let timeout_config = TimeoutConfig::default();
    let pool_config = PoolConfig::default();

    PipelineConfig::new(
        backend_state,
        subagent_backend,
        transformer_registry,
        timeout_config,
        pool_config,
    )
}

// =============================================================================
// Integration Test: Main Pipeline
// =============================================================================

#[tokio::test]
async fn test_main_pipeline_json_request() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"id": "msg_123", "model": "mock-sonnet", "content": [{"type": "text", "text": "Hello"}]}"#)).await;

    let config = create_integration_config(&mock.base_url());
    let backend_state = BackendState::from_config(config).unwrap();
    let pipeline_config = create_pipeline_config(backend_state);
    let mut ctx = create_pipeline_context();

    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"model": "claude-3-sonnet", "messages": [{"role": "user", "content": "Hi"}]}"#))
        .unwrap();

    let result = pipeline::execute_pipeline(
        req,
        &pipeline_config,
        &mut ctx,
        None, // backend_override
        None, // plugin_override
    ).await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response.status(), 200);

    // Verify request reached the mock backend
    let captured = mock.captured_requests().await;
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].path, "/v1/messages");

    // Verify model was rewritten
    let body: serde_json::Value = serde_json::from_slice(&captured[0].body).unwrap();
    assert_eq!(body["model"], "mock-sonnet"); // Should be rewritten to backend's model
}

#[tokio::test]
async fn test_main_pipeline_streaming_request() {
    let mock = MockBackend::start().await;

    // SSE streaming response
    let sse_events = vec![
        r#"{"type": "message_start", "message": {"id": "msg_123", "model": "mock-sonnet"}}"#,
        r#"{"type": "content_block_delta", "delta": {"type": "text", "text": "Hello"}}"#,
        r#"{"type": "message_stop"}"#,
    ];
    mock.enqueue_response(MockResponse::sse(&sse_events)).await;

    let config = create_integration_config(&mock.base_url());
    let backend_state = BackendState::from_config(config).unwrap();
    let pipeline_config = create_pipeline_config(backend_state);
    let mut ctx = create_pipeline_context();

    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"model": "claude-3-sonnet", "stream": true, "messages": []}"#))
        .unwrap();

    let result = pipeline::execute_pipeline(
        req,
        &pipeline_config,
        &mut ctx,
        None,
        None,
    ).await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response.status(), 200);

    // Check content-type is SSE
    let content_type = response.headers().get("content-type");
    assert!(content_type.is_some());
    assert!(content_type.unwrap().to_str().unwrap().contains("text/event-stream"));
}

#[tokio::test]
async fn test_main_pipeline_error_response() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::error(429, "Rate limit exceeded")).await;

    let config = create_integration_config(&mock.base_url());
    let backend_state = BackendState::from_config(config).unwrap();
    let pipeline_config = create_pipeline_config(backend_state);
    let mut ctx = create_pipeline_context();

    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"model": "claude-3", "messages": []}"#))
        .unwrap();

    let result = pipeline::execute_pipeline(
        req,
        &pipeline_config,
        &mut ctx,
        None,
        None,
    ).await;

    // Error responses should still be returned (not Err)
    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response.status(), 429);
}

// =============================================================================
// Integration Test: Teammate Pipeline
// =============================================================================

#[tokio::test]
async fn test_teammate_pipeline_no_thinking_session() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"ok": true}"#)).await;

    let config = create_integration_config(&mock.base_url());
    let backend_state = BackendState::from_config(config).unwrap();
    let pipeline_config = create_pipeline_config(backend_state);
    let mut ctx = create_pipeline_context();

    // Set up as teammate request
    ctx.span.record_mut().request_meta = Some(anyclaude::metrics::RequestMeta {
        method: "POST".to_string(),
        path: "/teammate/agent-1".to_string(),
        query: None,
        headers: None,
        body_preview: None,
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri("/teammate/agent-1")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"model": "claude-3", "messages": []}"#))
        .unwrap();

    let result = pipeline::execute_pipeline(
        req,
        &pipeline_config,
        &mut ctx,
        None,
        None,
    ).await;

    assert!(result.is_ok());
    // Teammate requests don't have thinking sessions, which is hard to verify
    // without introspection, but the request should succeed
}

// =============================================================================
// Integration Test: Backend Override
// =============================================================================

#[tokio::test]
async fn test_pipeline_with_backend_override() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"model": "override-backend"}"#)).await;

    let mut config = create_integration_config(&mock.base_url());
    // Add a second backend
    config.backends.push(Backend {
        name: "override".to_string(),
        display_name: "Override Backend".to_string(),
        base_url: mock.base_url(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: Some("override-model".to_string()),
        model_haiku: None,
    });

    let backend_state = BackendState::from_config(config).unwrap();
    let pipeline_config = create_pipeline_config(backend_state);
    let mut ctx = create_pipeline_context();

    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"model": "claude-3", "messages": []}"#))
        .unwrap();

    // Request with backend override
    let result = pipeline::execute_pipeline(
        req,
        &pipeline_config,
        &mut ctx,
        Some("override".to_string()), // backend_override
        None,
    ).await;

    assert!(result.is_ok());

    // Verify it went to the override backend
    let captured = mock.captured_requests().await;
    assert_eq!(captured.len(), 1);
}

// =============================================================================
// Integration Test: Model Mapping and Reverse Mapping
// =============================================================================

#[tokio::test]
async fn test_pipeline_model_rewrite_and_reverse_mapping() {
    let mock = MockBackend::start().await;

    // Response with the backend's model name
    mock.enqueue_response(MockResponse::json(r#"{"id": "msg_123", "model": "mock-sonnet", "content": []}"#)).await;

    let config = create_integration_config(&mock.base_url());
    let backend_state = BackendState::from_config(config).unwrap();
    let pipeline_config = create_pipeline_config(backend_state);
    let mut ctx = create_pipeline_context();

    // Request with original model name
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"model": "claude-3-5-sonnet-20241022", "messages": []}"#))
        .unwrap();

    let result = pipeline::execute_pipeline(
        req,
        &pipeline_config,
        &mut ctx,
        None,
        None,
    ).await;

    assert!(result.is_ok());

    // Verify request had model rewritten
    let captured = mock.captured_requests().await;
    let request_body: serde_json::Value = serde_json::from_slice(&captured[0].body).unwrap();
    assert_eq!(request_body["model"], "mock-sonnet");

    // Note: Reverse mapping happens in the response, which requires reading the body.
    // In integration tests with non-streaming responses, the model should be rewritten back.
}

// =============================================================================
// Integration Test: Thinking Compatibility
// =============================================================================

#[tokio::test]
async fn test_pipeline_thinking_compat_conversion() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"ok": true}"#)).await;

    let mut config = create_integration_config(&mock.base_url());
    // Enable thinking compatibility
    config.backends[0].thinking_compat = Some(true);
    config.backends[0].thinking_budget_tokens = Some(5000);

    let backend_state = BackendState::from_config(config).unwrap();
    let pipeline_config = create_pipeline_config(backend_state);
    let mut ctx = create_pipeline_context();

    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"model": "claude-3", "thinking": {"type": "adaptive"}, "messages": []}"#))
        .unwrap();

    let result = pipeline::execute_pipeline(
        req,
        &pipeline_config,
        &mut ctx,
        None,
        None,
    ).await;

    assert!(result.is_ok());

    // Verify thinking was converted
    let captured = mock.captured_requests().await;
    let body: serde_json::Value = serde_json::from_slice(&captured[0].body).unwrap();
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 5000);
}

// =============================================================================
// Integration Test: Headers Processing
// =============================================================================

#[tokio::test]
async fn test_pipeline_headers_stripping_and_addition() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"ok": true}"#)).await;

    let mut config = create_integration_config(&mock.base_url());
    // Use api_key auth to test header stripping
    config.backends[0].auth_type_str = "api_key".to_string();
    config.backends[0].api_key = Some("backend-secret-key".to_string());

    let backend_state = BackendState::from_config(config).unwrap();
    let pipeline_config = create_pipeline_config(backend_state);
    let mut ctx = create_pipeline_context();

    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header("authorization", "Bearer client-token") // Should be stripped
        .header("x-api-key", "client-api-key") // Should be stripped
        .header("x-custom", "preserve-me") // Should be preserved
        .body(Body::from(r#"{"model": "claude-3", "messages": []}"#))
        .unwrap();

    let result = pipeline::execute_pipeline(
        req,
        &pipeline_config,
        &mut ctx,
        None,
        None,
    ).await;

    assert!(result.is_ok());

    // Verify headers at backend
    let captured = mock.captured_requests().await;
    let headers = &captured[0].headers;

    // Client auth headers should be stripped
    let has_client_auth = headers.iter().any(|(k, v)| {
        (k.eq_ignore_ascii_case("authorization") || k.eq_ignore_ascii_case("x-api-key"))
            && v.contains("client-")
    });
    assert!(!has_client_auth, "Client auth headers should be stripped");

    // Backend's auth header should be present
    let has_backend_auth = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("x-api-key") && v == "backend-secret-key"
    });
    assert!(has_backend_auth, "Backend auth header should be present");

    // Custom header should be preserved
    let has_custom = headers.iter().any(|(k, v)| k == "x-custom" && v == "preserve-me");
    assert!(has_custom, "Custom header should be preserved");
}

// =============================================================================
// Corner Cases and Error Handling
// =============================================================================

#[tokio::test]
async fn test_pipeline_empty_request_body() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"ok": true}"#)).await;

    let config = create_integration_config(&mock.base_url());
    let backend_state = BackendState::from_config(config).unwrap();
    let pipeline_config = create_pipeline_config(backend_state);
    let mut ctx = create_pipeline_context();

    // GET request with empty body
    let req = Request::builder()
        .method(Method::GET)
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let result = pipeline::execute_pipeline(
        req,
        &pipeline_config,
        &mut ctx,
        None,
        None,
    ).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_pipeline_malformed_json_body() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"ok": true}"#)).await;

    let config = create_integration_config(&mock.base_url());
    let backend_state = BackendState::from_config(config).unwrap();
    let pipeline_config = create_pipeline_config(backend_state);
    let mut ctx = create_pipeline_context();

    // Request with malformed JSON but correct content-type
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"invalid json"#))
        .unwrap();

    let result = pipeline::execute_pipeline(
        req,
        &pipeline_config,
        &mut ctx,
        None,
        None,
    ).await;

    // Should succeed - malformed JSON is passed through as-is
    assert!(result.is_ok());

    // Verify raw body was forwarded
    let captured = mock.captured_requests().await;
    assert_eq!(captured[0].body, b"{\"invalid json");
}

#[tokio::test]
async fn test_pipeline_unconfigured_backend() {
    let mut config = create_integration_config("http://127.0.0.1:59999");
    // Backend with api_key auth but no key configured
    config.backends[0].auth_type_str = "api_key".to_string();
    config.backends[0].api_key = None; // Not configured

    let backend_state = BackendState::from_config(config).unwrap();
    let pipeline_config = create_pipeline_config(backend_state);
    let mut ctx = create_pipeline_context();

    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"model": "claude-3", "messages": []}"#))
        .unwrap();

    let result = pipeline::execute_pipeline(
        req,
        &pipeline_config,
        &mut ctx,
        None,
        None,
    ).await;

    // Should fail with backend not configured error
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("not configured"));
}
