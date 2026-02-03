//! Backend switching tests for the proxy.

mod common;

use anyclaude::config::{
    Backend, Config, ConfigStore, DebugLoggingConfig, Defaults, ProxyConfig, TerminalConfig,
    ThinkingConfig,
};
use anyclaude::proxy::ProxyServer;
use common::mock_backend::{MockBackend, MockResponse};
use reqwest::Client;
use std::path::PathBuf;
use std::time::Duration;

fn test_config_with_backends(backends: Vec<Backend>, bind_addr: &str) -> Config {
    Config {
        defaults: Defaults {
            active: backends.first().map(|b| b.name.clone()).unwrap_or_default(),
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
        debug_logging: DebugLoggingConfig::default(),
        backends,
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
    }
}

#[tokio::test]
async fn test_request_routed_to_active_backend() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"backend": "alpha"}"#)).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let config = test_config_with_backends(
        vec![create_backend("alpha", &mock.base_url())],
        &bind_addr,
    );
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
        .body(r#"{"test": true}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let requests = mock.captured_requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/messages");
}

#[tokio::test]
async fn test_backend_switch_routes_to_new_backend() {
    let mock_alpha = MockBackend::start().await;
    let mock_beta = MockBackend::start().await;

    mock_alpha.enqueue_response(MockResponse::json(r#"{"from": "alpha"}"#)).await;
    mock_beta.enqueue_response(MockResponse::json(r#"{"from": "beta"}"#)).await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let config = test_config_with_backends(
        vec![
            create_backend("alpha", &mock_alpha.base_url()),
            create_backend("beta", &mock_beta.base_url()),
        ],
        &bind_addr,
    );
    let config_store = ConfigStore::new(config.clone(), PathBuf::from("/tmp/test.toml"));
    let mut server = ProxyServer::new(config_store.clone()).unwrap();
    let backend_state = server.backend_state();

    // Bind to port before spawning - this prevents race conditions
    let (proxy_addr, _base_url) = server.try_bind(&config_store).await.unwrap();

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::new();

    // First request goes to alpha
    let resp1 = client
        .post(format!("http://{}/v1/messages", proxy_addr))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);
    let body1 = resp1.text().await.unwrap();
    assert!(body1.contains("alpha"));

    // Switch to beta
    backend_state.switch_backend("beta").unwrap();

    // Second request goes to beta
    let resp2 = client
        .post(format!("http://{}/v1/messages", proxy_addr))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);
    let body2 = resp2.text().await.unwrap();
    assert!(body2.contains("beta"));

    // Verify routing
    assert_eq!(mock_alpha.captured_requests().await.len(), 1);
    assert_eq!(mock_beta.captured_requests().await.len(), 1);
}

#[tokio::test]
async fn test_switch_to_nonexistent_backend_fails() {
    let mock = MockBackend::start().await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let config = test_config_with_backends(
        vec![create_backend("alpha", &mock.base_url())],
        &bind_addr,
    );
    let config_store = ConfigStore::new(config, PathBuf::from("/tmp/test.toml"));
    let server = ProxyServer::new(config_store).unwrap();
    let backend_state = server.backend_state();

    let result = backend_state.switch_backend("nonexistent");
    assert!(result.is_err());
    assert_eq!(backend_state.get_active_backend(), "alpha");
}

#[tokio::test]
async fn test_list_backends() {
    let mock_alpha = MockBackend::start().await;
    let mock_beta = MockBackend::start().await;

    let bind_addr = format!("127.0.0.1:{}", common::free_port());
    let config = test_config_with_backends(
        vec![
            create_backend("alpha", &mock_alpha.base_url()),
            create_backend("beta", &mock_beta.base_url()),
        ],
        &bind_addr,
    );
    let config_store = ConfigStore::new(config, PathBuf::from("/tmp/test.toml"));
    let server = ProxyServer::new(config_store).unwrap();
    let backend_state = server.backend_state();

    let backends = backend_state.list_backends();
    assert_eq!(backends.len(), 2);
    assert!(backends.contains(&"alpha".to_string()));
    assert!(backends.contains(&"beta".to_string()));
}
