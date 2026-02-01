//! SSE streaming tests for the proxy.

mod common;

use claudewrapper::config::{Backend, Config, ConfigStore, Defaults, ProxyConfig, TerminalConfig, ThinkingConfig};
use claudewrapper::proxy::ProxyServer;
use common::mock_backend::{MockBackend, MockResponse};
use reqwest::Client;
use std::path::PathBuf;
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
        thinking: ThinkingConfig::default(),
        terminal: TerminalConfig::default(),
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
    }
}

#[tokio::test]
async fn test_sse_streaming_passthrough() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::sse(&[
        r#"{"type":"content_block_start"}"#,
        r#"{"type":"content_block_delta"}"#,
        r#"{"type":"message_stop"}"#,
    ])).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let config = test_config(create_backend("test", &mock.base_url()), &bind_addr);
    let config_store = ConfigStore::new(config, PathBuf::from("/tmp/test.toml"));
    let server = ProxyServer::new(config_store).unwrap();
    let proxy_addr = server.addr;

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", proxy_addr))
        .body(r#"{"stream": true}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let content_type = resp.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(content_type.contains("text/event-stream"));

    let body = resp.text().await.unwrap();
    assert!(body.contains("content_block_start"));
    assert!(body.contains("message_stop"));
}

#[tokio::test]
async fn test_json_response_passthrough() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"content":[{"type":"text","text":"Hello"}]}"#)).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let config = test_config(create_backend("test", &mock.base_url()), &bind_addr);
    let config_store = ConfigStore::new(config, PathBuf::from("/tmp/test.toml"));
    let server = ProxyServer::new(config_store).unwrap();
    let proxy_addr = server.addr;

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", proxy_addr))
        .body(r#"{"stream": false}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let content_type = resp.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(content_type.contains("application/json"));

    let body = resp.text().await.unwrap();
    assert!(body.contains("Hello"));
}
