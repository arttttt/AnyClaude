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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_does_not_modify_request() {
        let transformer = NativeTransformer::new();
        let mut body = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "test thought", "signature": "sig"},
                    {"type": "text", "text": "response"}
                ]
            }]
        });

        let original = body.clone();
        let context = TransformContext::new("test", "req-1", "/v1/messages");
        let result = transformer.transform_request(&mut body, &context).unwrap();

        assert!(!result.changed);
        assert_eq!(body, original);
    }

    #[test]
    fn native_name() {
        let transformer = NativeTransformer::new();
        assert_eq!(transformer.name(), "native");
    }
}
