//! Error types for thinking transformation.

use thiserror::Error;

/// Errors that can occur during thinking block transformation.
#[derive(Debug, Error)]
pub enum TransformError {
    /// JSON parsing or serialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Summarization service failed
    #[error("Summarization failed: {0}")]
    Summarization(String),

    /// Backend not available for summarization
    #[error("Summarizer backend not available: {0}")]
    BackendUnavailable(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),
}
