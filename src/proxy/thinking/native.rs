//! Native transformer - passthrough relying on ThinkingRegistry.
//!
//! ThinkingRegistry handles session-based filtering in upstream.rs.
//! This transformer is a no-op passthrough.

use serde_json::Value;

use super::context::{TransformContext, TransformResult};
use super::error::TransformError;

/// Passthrough transformer relying on ThinkingRegistry for filtering.
#[derive(Debug, Default)]
pub struct NativeTransformer;

impl NativeTransformer {
    pub fn new() -> Self {
        Self
    }

    pub fn name(&self) -> &'static str {
        "native"
    }

    pub fn transform_request(
        &self,
        _body: &mut Value,
        _context: &TransformContext,
    ) -> Result<TransformResult, TransformError> {
        Ok(TransformResult::unchanged())
    }
}
