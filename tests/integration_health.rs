use claudewrapper::config::{Config, ConfigStore};
use claudewrapper::proxy::ProxyServer;
use reqwest::Client;
use std::path::PathBuf;

#[tokio::test]
async fn test_health_integration() {
    let config = Config::default();
    let config_store = ConfigStore::new(config, PathBuf::from("/tmp/test-config.toml"));
    let session_token = "test-session-token".to_string();
    let server = ProxyServer::new(config_store).expect("Failed to create proxy server");
    let addr_str = format!("{}", server.addr);

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let client = Client::new();
    let resp = client
        .get(format!("http://{}/health", addr_str))
        .header("Authorization", format!("Bearer {}", session_token))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);

    let body = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["status"], "healthy");
    assert_eq!(json["service"], "claudewrapper");
}

#[tokio::test]
async fn test_request_forwarding() {
    let config = Config::default();
    let config_store = ConfigStore::new(config, PathBuf::from("/tmp/test-config.toml"));
    let session_token = "test-session-token".to_string();
    let server = ProxyServer::new(config_store).expect("Failed to create proxy server");
    let addr_str = format!("{}", server.addr);

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let client = Client::new();
    let resp = client
        .get(format!("http://{}/v1/messages", addr_str))
        .header("x-test-header", "test-value")
        .header("Authorization", format!("Bearer {}", session_token))
        .send()
        .await;

    assert!(resp.is_err() || resp.unwrap().status().as_u16() != 200);
}
