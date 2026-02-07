//! Error types for thinking transformation.

use thiserror::Error;

/// Errors that can occur during thinking block transformation.
#[derive(Debug, Error)]
pub enum TransformError {
    /// JSON parsing or serialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
