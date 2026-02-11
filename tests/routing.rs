//! Tests for the proxy routing layer.

mod common;

use anyclaude::config::AgentTeamsConfig;
use anyclaude::proxy::routing::{build_rules, PathPrefixRule, RoutingRule};
use axum::body::Body;
use axum::http::Request;

fn make_request(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .body(Body::empty())
        .unwrap()
}

#[test]
fn path_prefix_rule_matches() {
    let rule = PathPrefixRule {
        prefix: "/teammate".to_string(),
        backend: "cheap".to_string(),
    };
    let req = make_request("/teammate/v1/messages");
    let action = rule.evaluate(&req).expect("should match");
    assert_eq!(action.backend, "cheap");
    assert_eq!(action.strip_prefix.as_deref(), Some("/teammate"));
}

#[test]
fn path_prefix_rule_exact_match() {
    let rule = PathPrefixRule {
        prefix: "/teammate".to_string(),
        backend: "cheap".to_string(),
    };
    let req = make_request("/teammate");
    assert!(rule.evaluate(&req).is_some());
}

#[test]
fn path_prefix_rule_no_match() {
    let rule = PathPrefixRule {
        prefix: "/teammate".to_string(),
        backend: "cheap".to_string(),
    };
    let req = make_request("/v1/messages");
    assert!(rule.evaluate(&req).is_none());
}

#[test]
fn path_prefix_rule_rejects_partial_segment() {
    let rule = PathPrefixRule {
        prefix: "/teammate".to_string(),
        backend: "cheap".to_string(),
    };
    // "/teammates" should NOT match "/teammate" — different path segment
    let req = make_request("/teammates/v1/messages");
    assert!(rule.evaluate(&req).is_none());
}

#[test]
fn build_rules_none_config() {
    let rules = build_rules(&None);
    assert!(rules.is_empty());
}

#[test]
fn build_rules_creates_prefix_rule() {
    let config = Some(AgentTeamsConfig {
        teammate_backend: "test-backend".to_string(),
    });
    let rules = build_rules(&config);
    assert_eq!(rules.len(), 1);

    let req = make_request("/teammate/v1/messages");
    let action = rules[0].evaluate(&req).expect("should match");
    assert_eq!(action.backend, "test-backend");
}

/// Test URI rewriting through the full middleware stack.
/// Verifies that the prefix is stripped and query string preserved.
#[tokio::test]
async fn middleware_rewrites_uri_and_sets_routed_to() {
    use anyclaude::proxy::routing::{routing_middleware, RoutedTo};
    use axum::extract::Extension;
    use axum::http::StatusCode;
    use axum::middleware;
    use axum::response::IntoResponse;
    use axum::routing::any;
    use axum::Router;
    use std::sync::Arc;

    // Handler that echoes the (rewritten) path and query
    async fn echo_handler(req: Request<Body>) -> impl IntoResponse {
        let routed = req.extensions().get::<RoutedTo>().cloned();
        let path = req.uri().path().to_string();
        let query = req.uri().query().unwrap_or("").to_string();
        let backend = routed.map(|r| r.backend).unwrap_or_default();
        format!("path={path} query={query} backend={backend}")
    }

    let rules: Vec<Box<dyn RoutingRule>> = vec![Box::new(PathPrefixRule {
        prefix: "/teammate".to_string(),
        backend: "cheap-backend".to_string(),
    })];

    let app = Router::new()
        .route("/{*path}", any(echo_handler))
        .layer(middleware::from_fn(routing_middleware))
        .layer(Extension(Arc::new(rules)));

    // Request with prefix + query string
    let req = Request::builder()
        .uri("/teammate/v1/messages?beta=true")
        .body(Body::empty())
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("path=/v1/messages"), "got: {text}");
    assert!(text.contains("query=beta=true"), "got: {text}");
    assert!(text.contains("backend=cheap-backend"), "got: {text}");

    // Request without prefix — no rewriting, no RoutedTo
    let req = Request::builder()
        .uri("/v1/messages")
        .body(Body::empty())
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("path=/v1/messages"), "got: {text}");
    assert!(text.contains("backend="), "should have empty backend, got: {text}");
    assert!(!text.contains("backend=cheap"), "got: {text}");
}

/// Bare prefix (no trailing path) should rewrite to "/".
#[tokio::test]
async fn middleware_rewrites_bare_prefix_to_root() {
    use anyclaude::proxy::routing::routing_middleware;
    use axum::extract::Extension;
    use axum::http::StatusCode;
    use axum::middleware;
    use axum::response::IntoResponse;
    use axum::routing::any;
    use axum::Router;
    use std::sync::Arc;

    async fn echo_path(req: Request<Body>) -> impl IntoResponse {
        req.uri().path().to_string()
    }

    let rules: Vec<Box<dyn RoutingRule>> = vec![Box::new(PathPrefixRule {
        prefix: "/teammate".to_string(),
        backend: "x".to_string(),
    })];

    let app = Router::new()
        .fallback(any(echo_path))
        .layer(middleware::from_fn(routing_middleware))
        .layer(Extension(Arc::new(rules)));

    let req = Request::builder()
        .uri("/teammate")
        .body(Body::empty())
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(String::from_utf8(body.to_vec()).unwrap(), "/");
}
