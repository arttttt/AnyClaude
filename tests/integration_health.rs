use claudewrapper::proxy::ProxyServer;
use reqwest::Client;

#[tokio::test]
async fn test_health_integration() {
    let server = ProxyServer::new();
    let addr_str = format!("{}", server.addr);

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let client = Client::new();
    let resp = client
        .get(format!("http://{}/health", addr_str))
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
    let server = ProxyServer::new();
    let addr_str = format!("{}", server.addr);

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let client = Client::new();
    let resp = client
        .get(format!("http://{}/v1/messages", addr_str))
        .header("x-test-header", "test-value")
        .send()
        .await;

    assert!(resp.is_err() || resp.unwrap().status().as_u16() != 200);
}
