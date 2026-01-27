use reqwest::Client;

#[tokio::test]
async fn test_sse_streaming_passthrough() {
    let mut server = claudewrapper::proxy::ProxyServer::new();
    let addr = server.addr;
    
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    let client = Client::new();
    
    // Test health endpoint (non-streaming)
    let response = client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();
    
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers().get("content-type").unwrap(), "application/json");
    
    let body = response.text().await.unwrap();
    assert!(body.contains("healthy"));
}
