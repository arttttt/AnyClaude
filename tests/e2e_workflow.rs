//! End-to-end workflow tests.

mod common;

use anyclaude::config::{
    Backend, Config, ConfigStore, DebugLoggingConfig, Defaults, ProxyConfig, TerminalConfig,
};
use anyclaude::metrics::DebugLogger;
use anyclaude::proxy::ProxyServer;
use common::mock_backend::{MockBackend, MockResponse};
use reqwest::Client;
use std::collections::HashMap;
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
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    }
}

#[tokio::test]
async fn test_full_lifecycle_health_request_shutdown() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"content":[{"type":"text","text":"Hi"}]}"#)).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let config = test_config(create_backend("test", &mock.base_url()), &bind_addr);
    let config_store = ConfigStore::new(config.clone(), PathBuf::from("/tmp/test.toml"));
    let debug_logger = Arc::new(DebugLogger::new(Default::default()));
    let mut server = ProxyServer::new(config_store.clone(), debug_logger).unwrap();

    // Bind to port before spawning - this prevents race conditions
    let (proxy_addr, _base_url) = server.try_bind(&config_store).await.unwrap();
    let handle = server.handle();

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::new();

    // 1. Health check
    let health = client
        .get(format!("http://{}/health", proxy_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    let health_body = health.text().await.unwrap();
    assert!(health_body.contains("healthy"));

    // 2. API request
    let api_resp = client
        .post(format!("http://{}/v1/messages", proxy_addr))
        .body(r#"{"messages":[{"role":"user","content":"Hi"}]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(api_resp.status(), 200);

    // 3. Graceful shutdown
    handle.shutdown();

    // Give time for shutdown
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn test_multiple_concurrent_requests() {
    let mock = MockBackend::start().await;
    // Enqueue 5 responses
    for i in 0..5 {
        mock.enqueue_response(MockResponse::json(&format!(r#"{{"id":{}}}"#, i))).await;
    }

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let config = test_config(create_backend("test", &mock.base_url()), &bind_addr);
    let config_store = ConfigStore::new(config.clone(), PathBuf::from("/tmp/test.toml"));
    let debug_logger = Arc::new(DebugLogger::new(Default::default()));
    let mut server = ProxyServer::new(config_store.clone(), debug_logger).unwrap();

    // Bind to port before spawning - this prevents race conditions
    let (proxy_addr, _base_url) = server.try_bind(&config_store).await.unwrap();

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::new();

    // Send 5 concurrent requests
    let mut handles = vec![];
    for _ in 0..5 {
        let c = client.clone();
        let addr = proxy_addr;
        handles.push(tokio::spawn(async move {
            c.post(format!("http://{}/v1/messages", addr))
                .body("{}")
                .send()
                .await
        }));
    }

    // All should succeed
    for h in handles {
        let resp = h.await.unwrap().unwrap();
        assert_eq!(resp.status(), 200);
    }

    assert_eq!(mock.captured_requests().await.len(), 5);
}
