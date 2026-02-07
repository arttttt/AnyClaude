//! Retry logic tests for the proxy.

mod common;

use anyclaude::config::{
    Backend, Config, ConfigStore, DebugLoggingConfig, Defaults, ProxyConfig, TerminalConfig,
};
use anyclaude::proxy::ProxyServer;
use common::mock_backend::{MockBackend, MockResponse};
use reqwest::Client;
use std::path::PathBuf;
use std::time::Duration;

fn test_config(backend: Backend, bind_addr: &str) -> Config {
    Config {
        defaults: Defaults {
            active: backend.name.clone(),
            timeout_seconds: 2,
            connect_timeout_seconds: 1,
            idle_timeout_seconds: 30,
            pool_idle_timeout_seconds: 30,
            pool_max_idle_per_host: 2,
            max_retries: 2,
            retry_backoff_base_ms: 50,
        },
        proxy: ProxyConfig {
            bind_addr: bind_addr.to_string(),
            base_url: format!("http://{}", bind_addr),
        },

        terminal: TerminalConfig::default(),
        debug_logging: DebugLoggingConfig::default(),
        backends: vec![backend],
    }
}

fn create_backend(name: &str, base_url: &str) -> Backend {
    Backend {
        name: name.to_string(),
        display_name: name.to_uppercase(),
        base_url: base_url.to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
    }
}

#[tokio::test]
async fn test_successful_request_no_retry() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"ok": true}"#)).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let config = test_config(create_backend("test", &mock.base_url()), &bind_addr);
    let config_store = ConfigStore::new(config.clone(), PathBuf::from("/tmp/test.toml"));
    let mut server = ProxyServer::new(config_store.clone()).unwrap();

    // Bind to port before spawning - this prevents race conditions
    let (proxy_addr, _base_url) = server.try_bind(&config_store).await.unwrap();

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", proxy_addr))
        .body("{}")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(mock.captured_requests().await.len(), 1);
}

#[tokio::test]
async fn test_error_response_not_retried() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::error(500, "Internal error")).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let config = test_config(create_backend("test", &mock.base_url()), &bind_addr);
    let config_store = ConfigStore::new(config.clone(), PathBuf::from("/tmp/test.toml"));
    let mut server = ProxyServer::new(config_store.clone()).unwrap();

    // Bind to port before spawning - this prevents race conditions
    let (proxy_addr, _base_url) = server.try_bind(&config_store).await.unwrap();

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", proxy_addr))
        .body("{}")
        .send()
        .await
        .unwrap();

    // Error responses are passed through, not retried
    assert_eq!(resp.status(), 500);
    assert_eq!(mock.captured_requests().await.len(), 1);
}

#[tokio::test]
async fn test_slow_response_succeeds() {
    let mock = MockBackend::start().await;
    // Response with 500ms delay (within 2s timeout)
    mock.enqueue_response(MockResponse::json(r#"{"slow": true}"#).with_delay(500)).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let config = test_config(create_backend("test", &mock.base_url()), &bind_addr);
    let config_store = ConfigStore::new(config.clone(), PathBuf::from("/tmp/test.toml"));
    let mut server = ProxyServer::new(config_store.clone()).unwrap();

    // Bind to port before spawning - this prevents race conditions
    let (proxy_addr, _base_url) = server.try_bind(&config_store).await.unwrap();

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", proxy_addr))
        .body("{}")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("slow"));
}
