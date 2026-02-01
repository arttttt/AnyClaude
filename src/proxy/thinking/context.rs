//! Context and result types for thinking transformation.

/// Context provided to transformers for each request.
#[derive(Debug, Clone)]
pub struct TransformContext {
    /// Current backend name
    pub backend: String,
    /// Request ID for tracing
    pub request_id: String,
}

impl TransformContext {
    pub fn new(backend: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self {
            backend: backend.into(),
            request_id: request_id.into(),
        }
    }
}

/// Result of a transformation operation.
#[derive(Debug, Default)]
pub struct TransformResult {
    /// Whether the request body was modified
    pub changed: bool,
    /// Transformation statistics
    pub stats: TransformStats,
}

/// Statistics about what was transformed.
#[derive(Debug, Default)]
pub struct TransformStats {
    /// Number of thinking blocks stripped/removed
    pub stripped_count: u32,
    /// Number of thinking blocks summarized
    pub summarized_count: u32,
    /// Number of tool_use blocks processed
    pub tools_processed_count: u32,
    /// Number of blocks passed through unchanged
    pub passthrough_count: u32,
}

impl TransformResult {
    /// Create a result indicating no changes were made.
    pub fn unchanged() -> Self {
        Self::default()
    }

    /// Create a result indicating changes were made.
    pub fn with_stats(stats: TransformStats) -> Self {
        let changed = stats.stripped_count > 0
            || stats.summarized_count > 0
            || stats.tools_processed_count > 0;
        Self { changed, stats }
    }
}
