//! Tests for session token authentication.
//!
//! These tests verify that:
//! 1. ANTHROPIC_CUSTOM_HEADERS is correctly set with the session token
//! 2. EnvSet builder correctly adds the session token
//! 3. The session token flows through spawn and restart params

use anyclaude::args::{build_restart_params, build_spawn_params, EnvSet};
use anyclaude::config::ClaudeSettingsManager;

/// Test that ANTHROPIC_CUSTOM_HEADERS is set in spawn env
#[test]
fn spawn_env_contains_custom_headers_with_session_token() {
    let args: Vec<String> = vec!["--model".into(), "opus".into()];
    let proxy_url = "http://127.0.0.1:4000";
    let session_token = "test-session-token-abc123";

    let params = build_spawn_params(
        &args,
        proxy_url,
        session_token,
        &ClaudeSettingsManager::new(),
        None,
        false, // subagent_routing
    );

    // Check that ANTHROPIC_CUSTOM_HEADERS contains the session token
    let custom_headers = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_CUSTOM_HEADERS")
        .map(|(_, v)| v.clone());

    assert!(
        custom_headers.is_some(),
        "ANTHROPIC_CUSTOM_HEADERS should be present in env"
    );

    let headers_value = custom_headers.unwrap();
    assert!(
        headers_value.contains("x-session-token"),
        "ANTHROPIC_CUSTOM_HEADERS should contain x-session-token"
    );
    assert!(
        headers_value.contains(session_token),
        "ANTHROPIC_CUSTOM_HEADERS should contain the session token value"
    );
}

/// Test that restart env also contains ANTHROPIC_CUSTOM_HEADERS
#[test]
fn restart_env_contains_custom_headers_with_session_token() {
    let args: Vec<String> = vec!["--resume".into(), "session123".into()];
    let proxy_url = "http://127.0.0.1:4000";
    let session_token = "test-session-token-xyz789";

    let params = build_restart_params(
        &args,
        proxy_url,
        session_token,
        &ClaudeSettingsManager::new(),
        None,
        false, // subagent_routing
        vec![],
        vec![],
    );

    // Check that ANTHROPIC_CUSTOM_HEADERS contains the session token
    let custom_headers = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_CUSTOM_HEADERS")
        .map(|(_, v)| v.clone());

    assert!(
        custom_headers.is_some(),
        "ANTHROPIC_CUSTOM_HEADERS should be present in restart env"
    );

    let headers_value = custom_headers.unwrap();
    assert!(
        headers_value.contains("x-session-token"),
        "ANTHROPIC_CUSTOM_HEADERS should contain x-session-token in restart"
    );
    assert!(
        headers_value.contains(session_token),
        "ANTHROPIC_CUSTOM_HEADERS should contain the session token value in restart"
    );
}

/// Test that EnvSet builder correctly adds session token
#[test]
fn env_set_with_session_token() {
    let env = EnvSet::new()
        .with_proxy_url("http://127.0.0.1:4000")
        .with_session_token("test-token-123")
        .build();

    // Check ANTHROPIC_BASE_URL
    assert!(env.iter().any(|(k, v)| k == "ANTHROPIC_BASE_URL" && v == "http://127.0.0.1:4000"));

    // Check ANTHROPIC_CUSTOM_HEADERS
    let custom_headers = env.iter().find(|(k, _)| k == "ANTHROPIC_CUSTOM_HEADERS");
    assert!(custom_headers.is_some(), "ANTHROPIC_CUSTOM_HEADERS should be present");
    assert_eq!(custom_headers.unwrap().1, "x-session-token:test-token-123");
}

/// Test that spawn env has both ANTHROPIC_BASE_URL and ANTHROPIC_CUSTOM_HEADERS
#[test]
fn spawn_env_has_all_required_vars() {
    let args: Vec<String> = vec![];
    let proxy_url = "http://127.0.0.1:4000";
    let session_token = "test-token";

    let params = build_spawn_params(
        &args,
        proxy_url,
        session_token,
        &ClaudeSettingsManager::new(),
        None,
        false, // subagent_routing
    );

    // Should have ANTHROPIC_BASE_URL
    assert!(
        params.env.iter().any(|(k, v)| k == "ANTHROPIC_BASE_URL" && v == proxy_url),
        "ANTHROPIC_BASE_URL should be set"
    );

    // Should have ANTHROPIC_CUSTOM_HEADERS with token
    assert!(
        params.env.iter().any(|(k, v)| k == "ANTHROPIC_CUSTOM_HEADERS" && v.contains(session_token)),
        "ANTHROPIC_CUSTOM_HEADERS should contain session token"
    );
}

/// Test that restart preserves all env vars
#[test]
fn restart_env_has_all_required_vars() {
    let args: Vec<String> = vec![];
    let proxy_url = "http://127.0.0.1:4000";
    let session_token = "test-token";

    let params = build_restart_params(
        &args,
        proxy_url,
        session_token,
        &ClaudeSettingsManager::new(),
        None,
        false, // subagent_routing
        vec![],
        vec![],
    );

    // Should have ANTHROPIC_BASE_URL
    assert!(
        params.env.iter().any(|(k, v)| k == "ANTHROPIC_BASE_URL" && v == proxy_url),
        "ANTHROPIC_BASE_URL should be set in restart"
    );

    // Should have ANTHROPIC_CUSTOM_HEADERS with token
    assert!(
        params.env.iter().any(|(k, v)| k == "ANTHROPIC_CUSTOM_HEADERS" && v.contains(session_token)),
        "ANTHROPIC_CUSTOM_HEADERS should contain session token in restart"
    );
}

/// Test that different tokens produce different custom headers
#[test]
fn different_tokens_produce_different_headers() {
    let env1 = EnvSet::new()
        .with_proxy_url("http://127.0.0.1:4000")
        .with_session_token("token-1")
        .build();

    let env2 = EnvSet::new()
        .with_proxy_url("http://127.0.0.1:4000")
        .with_session_token("token-2")
        .build();

    let headers1 = env1.iter().find(|(k, _)| k == "ANTHROPIC_CUSTOM_HEADERS").unwrap().1.clone();
    let headers2 = env2.iter().find(|(k, _)| k == "ANTHROPIC_CUSTOM_HEADERS").unwrap().1.clone();

    assert_ne!(headers1, headers2, "Different tokens should produce different headers");
    assert!(headers1.contains("token-1"));
    assert!(headers2.contains("token-2"));
}

/// Test that session token format is correct (x-session-token:VALUE)
#[test]
fn session_token_format_is_correct() {
    let env = EnvSet::new()
        .with_proxy_url("http://127.0.0.1:4000")
        .with_session_token("my-test-token")
        .build();

    let custom_headers = env.iter().find(|(k, _)| k == "ANTHROPIC_CUSTOM_HEADERS").unwrap();
    assert_eq!(custom_headers.1, "x-session-token:my-test-token");
}

// --- Middleware integration tests (real HTTP against ProxyServer) ---

use anyclaude::config::{Config, ConfigStore};
use anyclaude::metrics::DebugLogger;
use anyclaude::proxy::ProxyServer;
use std::path::PathBuf;
use std::sync::Arc;

async fn start_server_with_token(token: Option<String>) -> String {
    let config = Config::default();
    let config_store = ConfigStore::new(config, PathBuf::from("/tmp/test-session-auth.toml"));
    let debug_logger = Arc::new(DebugLogger::new(Default::default()));
    let mut server = ProxyServer::new(config_store.clone(), debug_logger, token)
        .expect("Failed to create proxy server");
    let (addr, _) = server.try_bind(&config_store).await.expect("Failed to bind");
    let addr_str = format!("{}", addr);
    tokio::spawn(async move { let _ = server.run().await; });
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    addr_str
}

/// Request without x-session-token header → 401 with our error message
#[tokio::test]
async fn middleware_rejects_missing_token() {
    let addr = start_server_with_token(Some("secret-token".into())).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/messages", addr))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let body = resp.text().await.unwrap();
    assert!(body.contains("invalid session token"), "Expected our auth error, got: {}", body);
}

/// Request with wrong x-session-token → 401 with our error message
#[tokio::test]
async fn middleware_rejects_wrong_token() {
    let addr = start_server_with_token(Some("secret-token".into())).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/messages", addr))
        .header("x-session-token", "wrong-token")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let body = resp.text().await.unwrap();
    assert!(body.contains("invalid session token"), "Expected our auth error, got: {}", body);
}

/// Request with correct x-session-token → passes auth (response is NOT our 401)
#[tokio::test]
async fn middleware_accepts_correct_token() {
    let addr = start_server_with_token(Some("secret-token".into())).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/messages", addr))
        .header("x-session-token", "secret-token")
        .body("{}")
        .send()
        .await
        .unwrap();
    // May get errors from downstream (no real backend), but NOT our auth rejection
    let body = resp.text().await.unwrap();
    assert!(
        !body.contains("invalid session token"),
        "Should pass auth middleware, got: {}",
        body,
    );
}

/// /health without token → 200 (health is not auth-protected)
#[tokio::test]
async fn health_endpoint_bypasses_auth() {
    let addr = start_server_with_token(Some("secret-token".into())).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

/// session_token=None → all requests pass without auth
#[tokio::test]
async fn no_token_configured_allows_all_requests() {
    let addr = start_server_with_token(None).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/messages", addr))
        .body("{}")
        .send()
        .await
        .unwrap();
    // May get errors from downstream (no real backend), but NOT our auth rejection
    let body = resp.text().await.unwrap();
    assert!(
        !body.contains("invalid session token"),
        "Should skip auth entirely, got: {}",
        body,
    );
}
