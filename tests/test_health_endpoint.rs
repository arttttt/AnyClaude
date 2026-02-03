#[tokio::test]
async fn test_health_endpoint() {
    use anyclaude::proxy::health::HealthHandler;
    use http_body_util::BodyExt;

    let handler = HealthHandler::new();
    let resp = handler.handle().await;

    assert_eq!(resp.status().as_u16(), 200);

    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8_lossy(&body_bytes);

    assert!(body_str.contains("healthy"));
    assert!(body_str.contains("anyclaude"));
}
