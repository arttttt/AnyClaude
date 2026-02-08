mod common;

use anyclaude::proxy::error::{ErrorResponse, ProxyError};
use axum::http::StatusCode;

#[test]
fn test_backend_not_found_status_code() {
    let err = ProxyError::BackendNotFound {
        backend: "missing".to_string(),
    };
    assert_eq!(err.status_code(), StatusCode::BAD_GATEWAY);
    assert_eq!(err.error_type(), "backend_not_found");
}

#[test]
fn test_request_timeout_status_code() {
    let err = ProxyError::RequestTimeout { duration: 30 };
    assert_eq!(err.status_code(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(err.error_type(), "request_timeout");
}

#[test]
fn test_error_response_format() {
    let err = ProxyError::BackendNotFound {
        backend: "test".to_string(),
    };
    let response = ErrorResponse::from_error(&err, "test-id-123");

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response.headers().get("Content-Type").unwrap(),
        "application/json"
    );
}
