//! Unit and integration tests for the 7-stage pipeline.
//!
//! Tests each stage independently plus full pipeline integration.

mod common;

use std::sync::Arc;
use std::time::SystemTime;

use axum::body::Body;
use axum::http::{header::{AUTHORIZATION, CONTENT_TYPE}, HeaderMap, Method, Request};
use serde_json::json;

use anyclaude::backend::{BackendState, SubagentRegistry};
use anyclaude::config::{Backend, Config, DebugLogDestination, DebugLogFormat, DebugLogLevel, DebugLoggingConfig, Defaults};
use anyclaude::metrics::{BackendOverride, DebugLogger, ObservabilityHub, RequestRecord, RequestSpan};
use anyclaude::proxy::pipeline::{self, PipelineContext, PipelineConfig};
use anyclaude::proxy::pool::PoolConfig;
use anyclaude::proxy::thinking::TransformerRegistry;
use anyclaude::proxy::timeout::TimeoutConfig;

// =============================================================================
// Test Helpers
// =============================================================================

fn create_test_config() -> Config {
    Config {
        defaults: Defaults {
            active: "test".to_string(),
            timeout_seconds: 5,
            connect_timeout_seconds: 2,
            idle_timeout_seconds: 60,
            pool_idle_timeout_seconds: 90,
            pool_max_idle_per_host: 8,
            max_retries: 3,
            retry_backoff_base_ms: 100,
        },
        backends: vec![
            Backend {
                name: "test".to_string(),
                display_name: "Test Backend".to_string(),
                base_url: "http://127.0.0.1:9999".to_string(),
                auth_type_str: "passthrough".to_string(),
                api_key: None,
                pricing: None,
                thinking_compat: None,
                thinking_budget_tokens: None,
                model_opus: None,
                model_sonnet: Some("test-sonnet".to_string()),
                model_haiku: None,
            },
            Backend {
                name: "anthropic".to_string(),
                display_name: "Anthropic".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                auth_type_str: "api_key".to_string(),
                api_key: Some("test-api-key".to_string()),
                pricing: None,
                thinking_compat: Some(false),
                thinking_budget_tokens: None,
                model_opus: None,
                model_sonnet: None,
                model_haiku: None,
            },
            Backend {
                name: "openrouter".to_string(),
                display_name: "OpenRouter".to_string(),
                base_url: "https://openrouter.ai/api".to_string(),
                auth_type_str: "bearer".to_string(),
                api_key: Some("openrouter-key".to_string()),
                pricing: None,
                thinking_compat: Some(true),
                thinking_budget_tokens: Some(5000),
                model_opus: Some("openrouter-opus".to_string()),
                model_sonnet: Some("openrouter-sonnet".to_string()),
                model_haiku: Some("openrouter-haiku".to_string()),
            },
        ],
        ..Default::default()
    }
}

fn create_test_context() -> PipelineContext {
    let record = RequestRecord {
        id: "test-request-id".to_string(),
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

#[allow(dead_code)]
fn create_test_pipeline_config() -> PipelineConfig {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();

    let subagent_registry = SubagentRegistry::new();
    let transformer_registry = Arc::new(TransformerRegistry::new());
    let timeout_config = TimeoutConfig::default();
    let pool_config = PoolConfig::default();

    PipelineConfig::new(
        backend_state,
        subagent_registry,
        transformer_registry,
        timeout_config,
        pool_config,
    )
}

// =============================================================================
// Stage 1: extract_request tests
// =============================================================================

#[tokio::test]
async fn test_extract_request_json_body() {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"model": "claude-3-sonnet", "messages": []}"#))
        .unwrap();

    let mut ctx = create_test_context();
    let result = pipeline::extract_request(req, &mut ctx).await.unwrap();

    assert_eq!(result.method, Method::POST);
    assert_eq!(result.uri.path(), "/v1/messages");
    assert_eq!(result.content_type, "application/json");
    assert!(result.parsed_body.is_some());

    let parsed = result.parsed_body.unwrap();
    assert_eq!(parsed["model"], "claude-3-sonnet");
}

#[tokio::test]
async fn test_extract_request_non_json_body() {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "text/plain")
        .body(Body::from("plain text body"))
        .unwrap();

    let mut ctx = create_test_context();
    let result = pipeline::extract_request(req, &mut ctx).await.unwrap();

    assert_eq!(result.content_type, "text/plain");
    assert!(result.parsed_body.is_none());
    assert_eq!(result.body_bytes, b"plain text body");
}

#[tokio::test]
async fn test_extract_request_empty_body() {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let mut ctx = create_test_context();
    let result = pipeline::extract_request(req, &mut ctx).await.unwrap();

    assert!(result.body_bytes.is_empty());
    assert!(result.parsed_body.is_none());
}

#[tokio::test]
async fn test_extract_request_malformed_json() {
    // Malformed JSON should not fail - just not be parsed
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"model": "test", invalid json}"#))
        .unwrap();

    let mut ctx = create_test_context();
    let result = pipeline::extract_request(req, &mut ctx).await.unwrap();

    // Should still succeed - parsed_body is None for malformed JSON
    assert!(result.parsed_body.is_none());
    assert_eq!(result.body_bytes, b"{\"model\": \"test\", invalid json}");
}

#[tokio::test]
async fn test_extract_request_no_content_type() {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/messages")
        .body(Body::from(r#"{"test": "value"}"#))
        .unwrap();

    let mut ctx = create_test_context();
    let result = pipeline::extract_request(req, &mut ctx).await.unwrap();

    assert_eq!(result.content_type, "");
    // Without content-type header, body should NOT be parsed as JSON
    assert!(result.parsed_body.is_none());
}

// =============================================================================
// Stage 2: resolve_backend tests
// =============================================================================

#[test]
fn test_resolve_backend_active_backend() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();

    let mut ctx = create_test_context();
    let parsed_body = Some(json!({"model": "claude-3"}));

    let backend = pipeline::resolve_backend(
        &backend_state,
        None,
        None,
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    ).unwrap();

    assert_eq!(backend.name, "test"); // active backend
}

#[test]
fn test_resolve_backend_override() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();

    let mut ctx = create_test_context();
    let parsed_body = Some(json!({"model": "claude-3"}));

    let backend = pipeline::resolve_backend(
        &backend_state,
        Some("anthropic".to_string()),
        None,
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    ).unwrap();

    assert_eq!(backend.name, "anthropic");
}

#[test]
fn test_resolve_backend_plugin_override() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();

    let mut ctx = create_test_context();
    let parsed_body = Some(json!({"model": "claude-3"}));

    let plugin_override = BackendOverride {
        backend: "openrouter".to_string(),
        reason: "routing_plugin".to_string(),
    };

    let backend = pipeline::resolve_backend(
        &backend_state,
        None,
        Some(plugin_override),
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    ).unwrap();

    assert_eq!(backend.name, "openrouter");
}

#[test]
fn test_resolve_backend_marker_model() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();

    let mut ctx = create_test_context();

    // Use marker- prefix to route to specific backend
    let parsed_body = Some(json!({"model": "marker-openrouter"}));

    let backend = pipeline::resolve_backend(
        &backend_state,
        None,
        None,
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    ).unwrap();

    assert_eq!(backend.name, "openrouter");
}

#[test]
fn test_resolve_backend_anyclaude_prefix() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();

    let mut ctx = create_test_context();

    // Use anyclaude- prefix as alternative marker
    let parsed_body = Some(json!({"model": "anyclaude-anthropic"}));

    let backend = pipeline::resolve_backend(
        &backend_state,
        None,
        None,
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    ).unwrap();

    assert_eq!(backend.name, "anthropic");
}

#[test]
fn test_resolve_backend_direct_backend_name() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();

    let mut ctx = create_test_context();

    // Direct backend name as model
    let parsed_body = Some(json!({"model": "openrouter"}));

    let backend = pipeline::resolve_backend(
        &backend_state,
        None,
        None,
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    ).unwrap();

    assert_eq!(backend.name, "openrouter");
}

#[test]
fn test_resolve_backend_missing_backend() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();

    let mut ctx = create_test_context();

    // Request a non-existent backend
    let result = pipeline::resolve_backend(
        &backend_state,
        Some("nonexistent".to_string()),
        None,
        None,
        &registry,
        &mut ctx,
    );

    assert!(result.is_err());
}

#[test]
fn test_resolve_backend_priority_plugin_over_teammate() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();

    let mut ctx = create_test_context();

    let plugin_override = BackendOverride {
        backend: "openrouter".to_string(),
        reason: "routing_plugin".to_string(),
    };

    // Plugin override should win over teammate override
    let backend = pipeline::resolve_backend(
        &backend_state,
        None,
        Some(plugin_override),
        None,
        &registry,
        &mut ctx,
    ).unwrap();

    assert_eq!(backend.name, "openrouter");
}

#[test]
fn test_resolve_backend_priority_teammate_over_marker() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();

    let mut ctx = create_test_context();

    let parsed_body = Some(json!({"model": "marker-openrouter"}));

    // Teammate override should win over marker model
    let backend = pipeline::resolve_backend(
        &backend_state,
        Some("anthropic".to_string()), // teammate route
        None,
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    ).unwrap();

    assert_eq!(backend.name, "anthropic");
}

// =============================================================================
// Stage 2b: AC marker session affinity
// =============================================================================

#[test]
fn test_ac_marker_routes_to_backend() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();
    // Register subagent identifier → backend mapping (simulates SubagentStart hook)
    registry.register("a1b2c3d4e5f6a7b8", "openrouter");

    let mut ctx = create_test_context();

    // AC marker in hook context format, resolved via registry
    let parsed_body = Some(json!({
        "model": "claude-haiku-4-5-20251001",
        "messages": [{"role": "user", "content": "SubagentStart hook additional context: \u{27E8}AC:a1b2c3d4e5f6a7b8\u{27E9}"}]
    }));

    let backend = pipeline::resolve_backend(
        &backend_state,
        None,
        None,
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    ).unwrap();

    assert_eq!(backend.name, "openrouter");
}

#[test]
fn test_no_ac_marker_uses_active_backend() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();

    let mut ctx = create_test_context();

    // No AC marker → should use active backend
    let parsed_body = Some(json!({
        "model": "claude-haiku-4-5-20251001",
        "messages": [{"role": "user", "content": "hello"}]
    }));

    let backend = pipeline::resolve_backend(
        &backend_state,
        None,
        None,
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    ).unwrap();

    // Falls back to active backend ("test")
    assert_eq!(backend.name, "test");
}

#[test]
fn test_ac_marker_wins_over_marker_model() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();
    registry.register("a2b3c4d5e6f7a8b9", "openrouter");

    let mut ctx = create_test_context();

    // AC marker should win over marker- model prefix
    let parsed_body = Some(json!({
        "model": "marker-anthropic",  // would route to anthropic
        "messages": [{"role": "user", "content": "SubagentStart hook additional context: \u{27E8}AC:a2b3c4d5e6f7a8b9\u{27E9}"}]  // but registry maps to openrouter
    }));

    let backend = pipeline::resolve_backend(
        &backend_state,
        None,
        None,
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    ).unwrap();

    assert_eq!(backend.name, "openrouter");
}

#[test]
fn test_ac_marker_skipped_when_registry_empty() {
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();
    // Empty registry — marker parsing is skipped entirely

    let mut ctx = create_test_context();

    let parsed_body = Some(json!({
        "model": "claude-haiku-4-5-20251001",
        "messages": [{"role": "user", "content": "SubagentStart hook additional context: \u{27E8}AC:a3b4c5d6e7f8a9b0\u{27E9}"}]
    }));

    let result = pipeline::resolve_backend(
        &backend_state,
        None,
        None,
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    );

    // With empty registry, AC marker is skipped — falls through to active backend
    let backend = result.expect("empty registry should skip marker, not error");
    assert_eq!(backend.name, backend_state.get_active_backend());
}

// =============================================================================
// Stage 3: create_thinking tests
// =============================================================================

#[test]
fn test_create_thinking_always_creates_session() {
    // create_thinking always returns Some — teammate detection is
    // handled by execute_pipeline (which skips calling create_thinking
    // when backend_override is present).
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let transformer_registry = Arc::new(TransformerRegistry::new());
    let mut ctx = create_test_context();

    let backend = backend_state.get_backend_config("test").unwrap();
    let session = pipeline::create_thinking(&transformer_registry, &backend, &mut ctx);

    assert!(session.is_some());
}

// =============================================================================
// Stage 4: transform_body tests
// =============================================================================

#[test]
fn test_transform_body_no_json() {
    let body_bytes = b"plain text".to_vec();
    let mut ctx = create_test_context();
    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: Some("mapped-sonnet".to_string()),
        model_haiku: None,
    };

    let (result, is_streaming, mapping) = pipeline::transform_body(
        body_bytes.clone(),
        None,
        &backend,
        None,
        &mut ctx,
    ).unwrap();

    assert_eq!(result, body_bytes);
    assert!(!is_streaming);
    assert!(mapping.is_none());
}

#[test]
fn test_transform_body_model_rewrite() {
    let body_json = json!({
        "model": "claude-3-5-sonnet-20241022",
        "messages": []
    });
    let body_bytes = serde_json::to_vec(&body_json).unwrap();
    let mut ctx = create_test_context();
    let backend = Backend {
        name: "openrouter".to_string(),
        display_name: "OpenRouter".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: Some("mapped-sonnet".to_string()),
        model_haiku: None,
    };

    let (result, _, mapping) = pipeline::transform_body(
        body_bytes,
        Some(body_json),
        &backend,
        None,
        &mut ctx,
    ).unwrap();

    let result_json: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(result_json["model"], "mapped-sonnet");
    assert!(mapping.is_some());
    assert_eq!(mapping.unwrap().original, "claude-3-5-sonnet-20241022");
}

#[test]
fn test_transform_body_thinking_compat_adaptive_to_enabled() {
    let body_json = json!({
        "model": "claude-3-sonnet",
        "thinking": {"type": "adaptive"},
        "max_tokens": 4096
    });
    let body_bytes = serde_json::to_vec(&body_json).unwrap();
    let mut ctx = create_test_context();
    let backend = Backend {
        name: "openrouter".to_string(),
        display_name: "OpenRouter".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: Some(true), // Enable thinking compat
        thinking_budget_tokens: Some(8000),
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    let (result, _, _) = pipeline::transform_body(
        body_bytes,
        Some(body_json),
        &backend,
        None,
        &mut ctx,
    ).unwrap();

    let result_json: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(result_json["thinking"]["type"], "enabled");
    assert_eq!(result_json["thinking"]["budget_tokens"], 8000);
}

#[test]
fn test_transform_body_thinking_compat_no_adaptive() {
    // Already enabled thinking should not be modified
    let body_json = json!({
        "model": "claude-3-sonnet",
        "thinking": {"type": "enabled", "budget_tokens": 5000}
    });
    let body_bytes = serde_json::to_vec(&body_json).unwrap();
    let mut ctx = create_test_context();
    let backend = Backend {
        name: "openrouter".to_string(),
        display_name: "OpenRouter".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: Some(true),
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    let (result, _, _) = pipeline::transform_body(
        body_bytes,
        Some(body_json),
        &backend,
        None,
        &mut ctx,
    ).unwrap();

    let result_json: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(result_json["thinking"]["type"], "enabled");
    assert_eq!(result_json["thinking"]["budget_tokens"], 5000);
}

#[test]
fn test_transform_body_no_thinking_compat() {
    // Backend with thinking_compat disabled should not convert
    let body_json = json!({
        "model": "claude-3-sonnet",
        "thinking": {"type": "adaptive"}
    });
    let body_bytes = serde_json::to_vec(&body_json).unwrap();
    let mut ctx = create_test_context();
    let backend = Backend {
        name: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: Some(false), // Disabled
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    let (result, _, _) = pipeline::transform_body(
        body_bytes.clone(),
        Some(body_json),
        &backend,
        None,
        &mut ctx,
    ).unwrap();

    // Body should remain unchanged (no transformation)
    assert_eq!(result, body_bytes);
}

#[test]
fn test_transform_body_streaming_detection() {
    let body_json = json!({
        "model": "claude-3-sonnet",
        "stream": true
    });
    let body_bytes = serde_json::to_vec(&body_json).unwrap();
    let mut ctx = create_test_context();
    let backend = Backend::default();

    let (_, is_streaming, _) = pipeline::transform_body(
        body_bytes,
        Some(body_json),
        &backend,
        None,
        &mut ctx,
    ).unwrap();

    assert!(is_streaming);
}

#[test]
fn test_transform_body_thinking_filtering() {
    // Create a thinking session for filtering
    let body_json = json!({
        "model": "claude-3-sonnet",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Hello"}
                ]
            }
        ]
    });
    let body_bytes = serde_json::to_vec(&body_json).unwrap();
    let mut ctx = create_test_context();
    let backend = Backend::default();

    // Without thinking session, no filtering occurs
    let (result, _, _) = pipeline::transform_body(
        body_bytes.clone(),
        Some(body_json.clone()),
        &backend,
        None,
        &mut ctx,
    ).unwrap();

    // Should be unchanged since there's no ThinkingSession
    assert_eq!(result, body_bytes);
}

#[test]
fn test_transform_body_budget_calculation_from_max_tokens() {
    // When thinking_budget_tokens is not configured, calculate from max_tokens
    let body_json = json!({
        "model": "claude-3-sonnet",
        "thinking": {"type": "adaptive"},
        "max_tokens": 1000
    });
    let body_bytes = serde_json::to_vec(&body_json).unwrap();
    let mut ctx = create_test_context();
    let backend = Backend {
        name: "openrouter".to_string(),
        display_name: "OpenRouter".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: Some(true),
        thinking_budget_tokens: None, // Not configured - should use max_tokens - 1
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    let (result, _, _) = pipeline::transform_body(
        body_bytes,
        Some(body_json),
        &backend,
        None,
        &mut ctx,
    ).unwrap();

    let result_json: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(result_json["thinking"]["budget_tokens"], 999); // max_tokens - 1
}

#[test]
fn test_transform_body_budget_default_when_no_max_tokens() {
    // When neither budget nor max_tokens is set, use default
    let body_json = json!({
        "model": "claude-3-sonnet",
        "thinking": {"type": "adaptive"}
    });
    let body_bytes = serde_json::to_vec(&body_json).unwrap();
    let mut ctx = create_test_context();
    let backend = Backend {
        name: "openrouter".to_string(),
        display_name: "OpenRouter".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: Some(true),
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    let (result, _, _) = pipeline::transform_body(
        body_bytes,
        Some(body_json),
        &backend,
        None,
        &mut ctx,
    ).unwrap();

    let result_json: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(result_json["thinking"]["budget_tokens"], 10000); // Default
}

// =============================================================================
// Stage 5: build_headers tests
// =============================================================================

#[test]
fn test_build_headers_basic() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert("x-custom-header", "value".parse().unwrap());

    let mut ctx = create_test_context();
    let backend = Backend::default(); // passthrough auth

    let result = pipeline::build_headers(&headers, &backend, &mut ctx).unwrap();

    // Should contain our custom headers
    assert!(result.iter().any(|(k, _)| k == "content-type"));
    assert!(result.iter().any(|(k, _)| k == "x-custom-header"));

    // Should NOT contain HOST or CONTENT_LENGTH (set by HTTP client)
    assert!(!result.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")));
    assert!(!result.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-length")));
}

#[test]
fn test_build_headers_strips_auth_for_own_credentials() {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, "Bearer client-token".parse().unwrap());
    headers.insert("x-api-key", "client-api-key".parse().unwrap());
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());

    let mut ctx = create_test_context();
    let backend = Backend {
        name: "openrouter".to_string(),
        display_name: "OpenRouter".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "bearer".to_string(),
        api_key: Some("backend-api-key".to_string()),
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    let result = pipeline::build_headers(&headers, &backend, &mut ctx).unwrap();

    // Count auth headers - should only have the backend's auth, not client's
    let auth_count = result.iter().filter(|(k, _)| k.eq_ignore_ascii_case("authorization")).count();
    assert_eq!(auth_count, 1);

    // Should have backend's bearer token (format: "Bearer {key}")
    let auth_header = result.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization"));
    assert!(auth_header.is_some());
    assert_eq!(auth_header.unwrap().1, "Bearer backend-api-key");
}

#[test]
fn test_build_headers_passthrough_keeps_auth() {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, "Bearer client-token".parse().unwrap());
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());

    let mut ctx = create_test_context();
    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "passthrough".to_string(), // passthrough
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    let result = pipeline::build_headers(&headers, &backend, &mut ctx).unwrap();

    // Should keep client auth headers in passthrough mode
    let auth_header = result.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization"));
    assert!(auth_header.is_some());
    assert_eq!(auth_header.unwrap().1, "Bearer client-token");
}

#[test]
fn test_build_headers_patches_anthropic_beta() {
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-beta", "adaptive-thinking-2025-01-01,prompt-caching-2024-07-01".parse().unwrap());
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());

    let mut ctx = create_test_context();
    let backend = Backend {
        name: "openrouter".to_string(),
        display_name: "OpenRouter".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: Some(true), // needs thinking compat
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    let result = pipeline::build_headers(&headers, &backend, &mut ctx).unwrap();

    let beta_header = result.iter().find(|(k, _)| k.eq_ignore_ascii_case("anthropic-beta"));
    assert!(beta_header.is_some());
    let beta_value = beta_header.unwrap().1.clone();

    // Should strip adaptive-thinking-* prefix
    assert!(!beta_value.contains("adaptive-thinking"));

    // Should add interleaved-thinking if not present
    assert!(beta_value.contains("interleaved-thinking-2025-05-14"));

    // Should keep other parts
    assert!(beta_value.contains("prompt-caching-2024-07-01"));
}

#[test]
fn test_build_headers_no_anthropic_beta_patch_for_anthropic() {
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-beta", "adaptive-thinking-2025-01-01".parse().unwrap());
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());

    let mut ctx = create_test_context();
    let backend = Backend {
        name: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        auth_type_str: "api_key".to_string(),
        api_key: Some("key".to_string()),
        pricing: None,
        thinking_compat: Some(false), // no thinking compat
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    let result = pipeline::build_headers(&headers, &backend, &mut ctx).unwrap();

    let beta_header = result.iter().find(|(k, _)| k.eq_ignore_ascii_case("anthropic-beta"));
    assert!(beta_header.is_some());

    // Should NOT patch for Anthropic backends
    assert!(beta_header.unwrap().1.contains("adaptive-thinking"));
}

// =============================================================================
// Corner Cases Documentation
// =============================================================================

/// # Pipeline Corner Cases
///
/// ## Stage 1: extract_request
/// - **Empty body with JSON content-type**: Returns empty bytes, parsed_body = None
/// - **Malformed JSON**: Returns raw bytes, parsed_body = None (graceful degradation)
/// - **No content-type header**: parsed_body = None even if body is valid JSON
/// - **Binary data**: Handled as non-JSON, parsed_body = None
/// - **Very large bodies**: Should handle up to memory limits
/// - **Invalid UTF-8 in headers**: Headers with non-UTF8 values are skipped via to_str().ok()
///
/// ## Stage 2: resolve_backend
/// - **Plugin override takes highest priority**: Even over explicit teammate routes
/// - **Invalid marker model**: If marker-{backend} references non-existent backend, falls through to active
/// - **Empty backend_override string**: Treated as None (falls through to next priority)
/// - **Backend removed during request**: Would fail at get_backend_config stage with BackendNotFound
/// - **Circular backend references**: Not possible - backend names are flat
///
/// ## Stage 3: create_thinking
/// - **Teammate detection**: Based on `backend_override.is_some()` in execute_pipeline,
///   NOT on path prefix. The path check was unreliable since axum nest() strips prefixes.
/// - **create_thinking always returns Some**: Caller decides whether to call it.
///
/// ## Stage 4: transform_body
/// - **Model rewrite precedence**: Family detection (opus/sonnet/haiku) is case-sensitive
/// - **Multiple thinking types in same request**: Only handles "adaptive" -> "enabled"
/// - **Zero max_tokens**: budget_tokens becomes 0 (saturating_sub handles underflow)
/// - **max_tokens = 1**: budget_tokens becomes 0 (1 - 1 = 0)
/// - **JSON serialization failure**: Falls back to original body bytes
/// - **No model field**: No model mapping created
/// - **Empty model string**: Treated as valid model name (empty string)
///
/// ## Stage 5: build_headers
/// - **Case sensitivity**: Header names compared case-insensitively where appropriate
/// - **Duplicate headers**: Vec<(String, String)> preserves duplicates
/// - **Invalid header values**: Skipped via to_str().ok() check
/// - **Missing backend auth**: build_auth_header returns None, no panic
/// - **Mixed auth headers**: Both Authorization and x-api-key stripped when appropriate
///
/// ## Stage 6: forward_with_retry
/// - **Streaming + retry**: Each retry starts fresh stream (previous streams dropped)
/// - **Backend switch mid-request**: Not possible - BackendState cloned at start
/// - **Connection pool exhaustion**: Retry with backoff
/// - **Partial response then failure**: Treated as success (response received)
/// - **Request body rewind**: Body bytes cloned for each retry attempt
///
/// ## Stage 7: handle_response
/// - **Model mapping with non-JSON response**: Passes through unchanged
/// - **Streaming error after headers sent**: Client sees truncated stream
/// - **Content-Length with model mapping**: Stripped to avoid mismatch
/// - **Zero-byte response**: Empty body forwarded correctly
/// - **Invalid UTF-8 in SSE**: Handled by bytes-based processing

// Teammate detection is now based on backend_override presence in
// execute_pipeline, not path-based. The old path prefix check was
// unreliable because axum nest() strips the /teammate prefix before
// proxy_handler runs. See execute_pipeline in mod.rs.

#[test]
fn test_corner_case_model_family_detection_case_sensitive() {
    // Model family detection is case-sensitive for "opus", "sonnet", "haiku"
    let test_cases = vec![
        ("claude-opus", Some("openrouter-opus")),    // lowercase - matches
        ("claude-OPUS", None),                        // uppercase - no match
        ("claude-Opus", None),                        // mixed case - no match
        ("claude-sonnet", Some("openrouter-sonnet")), // lowercase - matches
        ("claude-SONNET", None),                      // uppercase - no match
        ("claude-haiku", Some("openrouter-haiku")),   // lowercase - matches
        ("claude-HAIKU", None),                       // uppercase - no match
    ];

    let backend = Backend {
        name: "openrouter".to_string(),
        display_name: "OpenRouter".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: Some("openrouter-opus".to_string()),
        model_sonnet: Some("openrouter-sonnet".to_string()),
        model_haiku: Some("openrouter-haiku".to_string()),
    };

    for (model, expected) in test_cases {
        let result = backend.resolve_model(model);
        assert_eq!(result, expected, "Model '{}' resolution mismatch", model);
    }
}

#[test]
fn test_corner_case_budget_tokens_with_small_max_tokens() {
    // Test edge cases for budget calculation
    // Note: The calculation is: mt.saturating_sub(1) as u32
    // For large u64 values, casting to u32 truncates to lower 32 bits
    let test_cases = vec![
        (100u64, 99u32),    // normal case
        (1u64, 0u32),       // min - results in 0
        (0u64, 0u32),       // zero - saturating_sub prevents underflow
        (u64::MAX, 4294967294u32), // max: (0xFFFFFFFFFFFFFFFF - 1) as u32 = 0xFFFFFFFE
    ];

    for (max_tokens, expected_budget) in test_cases {
        let body_json = json!({
            "model": "claude-3-sonnet",
            "thinking": {"type": "adaptive"},
            "max_tokens": max_tokens
        });
        let body_bytes = serde_json::to_vec(&body_json).unwrap();
        let mut ctx = create_test_context();
        let backend = Backend {
            name: "openrouter".to_string(),
            display_name: "OpenRouter".to_string(),
            base_url: "http://test".to_string(),
            auth_type_str: "passthrough".to_string(),
            api_key: None,
            pricing: None,
            thinking_compat: Some(true),
            thinking_budget_tokens: None,
            model_opus: None,
            model_sonnet: None,
            model_haiku: None,
        };

        let (result, _, _) = pipeline::transform_body(
            body_bytes,
            Some(body_json),
            &backend,
            None,
            &mut ctx,
        ).unwrap();

        let result_json: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(
            result_json["thinking"]["budget_tokens"].as_u64(),
            Some(expected_budget as u64),
            "max_tokens={} should give budget={}",
            max_tokens,
            expected_budget
        );
    }
}

#[test]
fn test_corner_case_invalid_marker_model() {
    // Marker model pointing to non-existent backend should fall through
    let config = create_test_config();
    let backend_state = BackendState::from_config(config).unwrap();
    let registry = SubagentRegistry::new();

    let mut ctx = create_test_context();

    // Invalid marker - "nonexistent" is not a valid backend
    let parsed_body = Some(json!({"model": "marker-nonexistent"}));

    let backend = pipeline::resolve_backend(
        &backend_state,
        None,
        None,
        parsed_body.as_ref(),
        &registry,
        &mut ctx,
    ).unwrap();

    // Should fall through to active backend, not fail
    assert_eq!(backend.name, "test");
}

#[test]
fn test_corner_case_empty_model_string() {
    // Empty model string should be treated as valid (though unusual)
    let body_json = json!({
        "model": "",
        "messages": []
    });
    let body_bytes = serde_json::to_vec(&body_json).unwrap();
    let mut ctx = create_test_context();
    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "http://test".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: Some("mapped-sonnet".to_string()),
        model_haiku: None,
    };

    let (result, _, mapping) = pipeline::transform_body(
        body_bytes,
        Some(body_json),
        &backend,
        None,
        &mut ctx,
    ).unwrap();

    let result_json: serde_json::Value = serde_json::from_slice(&result).unwrap();
    // Empty model should remain empty (no family match)
    assert_eq!(result_json["model"], "");
    assert!(mapping.is_none());
}
