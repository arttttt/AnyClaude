//! Error types and response handling for the proxy server.
//!
//! Provides structured error classification, HTTP status code mapping,
//! and JSON error response generation.

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
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
        source: reqwest::Error,
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

    /// HTTP error from request building
    #[error("HTTP error: {0}")]
    Http(String),
}

impl From<axum::http::Error> for ProxyError {
    fn from(err: axum::http::Error) -> Self {
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
    pub fn from_error(err: &ProxyError, request_id: &str) -> Response {
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
            .body(Body::from(body.to_string()))
            .expect("Failed to build error response")
    }
}
