//! Error types and response handling for the proxy server.
//!
//! Provides structured error classification, HTTP status code mapping,
//! and JSON error response generation.

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::{Response, StatusCode};
use thiserror::Error;

use crate::config::ConfigError;

/// Errors that can occur during proxy operations.
#[derive(Debug, Error)]
pub enum ProxyError {
    /// Configuration-related errors
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    /// Backend not found in configuration
    #[error("Backend '{backend}' not found")]
    BackendNotFound { backend: String },

    /// Backend exists but is not properly configured
    #[error("Backend '{backend}' not configured: {reason}")]
    BackendNotConfigured { backend: String, reason: String },

    /// Failed to connect to upstream server
    #[error("Connection failed to '{backend}': {source}")]
    ConnectionError {
        backend: String,
        #[source]
        source: hyper_util::client::legacy::Error,
    },

    /// Request exceeded total timeout
    #[error("Request timeout after {duration}s")]
    RequestTimeout { duration: u64 },

    /// Streaming response exceeded idle timeout
    #[error("Idle timeout after {duration}s of inactivity")]
    IdleTimeout { duration: u64 },

    /// Invalid request format or parameters
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Upstream returned an error response
    #[error("Upstream error: {status} - {message}")]
    UpstreamError { status: u16, message: String },

    /// Internal server error
    #[error("Internal error: {0}")]
    Internal(String),

    /// HTTP error from hyper
    #[error("HTTP error: {0}")]
    Http(String),
}

impl From<hyper::http::Error> for ProxyError {
    fn from(err: hyper::http::Error) -> Self {
        ProxyError::Http(err.to_string())
    }
}

impl ProxyError {
    /// Map error variant to appropriate HTTP status code
    pub fn status_code(&self) -> StatusCode {
        match self {
            ProxyError::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ProxyError::BackendNotFound { .. } => StatusCode::BAD_GATEWAY,
            ProxyError::BackendNotConfigured { .. } => StatusCode::BAD_GATEWAY,
            ProxyError::ConnectionError { .. } => StatusCode::BAD_GATEWAY,
            ProxyError::RequestTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
            ProxyError::IdleTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
            ProxyError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            ProxyError::UpstreamError { status, .. } => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY)
            }
            ProxyError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ProxyError::Http(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Get error type string for JSON responses
    pub fn error_type(&self) -> &'static str {
        match self {
            ProxyError::Config(_) => "config_error",
            ProxyError::BackendNotFound { .. } => "backend_not_found",
            ProxyError::BackendNotConfigured { .. } => "backend_not_configured",
            ProxyError::ConnectionError { .. } => "connection_error",
            ProxyError::RequestTimeout { .. } => "request_timeout",
            ProxyError::IdleTimeout { .. } => "idle_timeout",
            ProxyError::InvalidRequest(_) => "invalid_request",
            ProxyError::UpstreamError { .. } => "upstream_error",
            ProxyError::Internal(_) => "internal_error",
            ProxyError::Http(_) => "http_error",
        }
    }
}

/// Builder for standardized error responses
pub struct ErrorResponse;

impl ErrorResponse {
    /// Create a JSON error response from a ProxyError
    pub fn from_error(err: &ProxyError, request_id: &str) -> Response<Full<Bytes>> {
        let body = serde_json::json!({
            "error": {
                "type": err.error_type(),
                "message": err.to_string(),
                "request_id": request_id
            }
        });

        Response::builder()
            .status(err.status_code())
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(body.to_string())))
            .expect("Failed to build error response")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
