//! Context and result types for thinking transformation.

/// Context provided to transformers for each request.
#[derive(Debug, Clone)]
pub struct TransformContext {
    /// Current backend name
    pub backend: String,
    /// Request ID for tracing
    pub request_id: String,
    /// Request path (e.g., "/v1/messages")
    pub request_path: String,
}

impl TransformContext {
    pub fn new(
        backend: impl Into<String>,
        request_id: impl Into<String>,
        request_path: impl Into<String>,
    ) -> Self {
        Self {
            backend: backend.into(),
            request_id: request_id.into(),
            request_path: request_path.into(),
        }
    }
}

/// Result of a transformation operation.
#[derive(Debug, Default)]
pub struct TransformResult {
    /// Whether the request body was modified
    pub changed: bool,
}

impl TransformResult {
    /// Create a result indicating no changes were made.
    pub fn unchanged() -> Self {
        Self::default()
    }
}
