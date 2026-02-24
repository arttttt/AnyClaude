//! Stage 3: Create thinking session.
//!
//! Creates a ThinkingSession for the request AFTER backend resolution.
//! This replaces the thinking_middleware approach.

use std::sync::Arc;

use crate::config::Backend;
use crate::proxy::thinking::{ThinkingSession, TransformerRegistry};
use crate::proxy::pipeline::PipelineContext;

/// Stage 3: Create thinking session.
///
/// Creates a ThinkingSession for main agent requests. The caller is
/// responsible for skipping this stage for teammate requests (those
/// with a backend_override).
///
/// The session is created AFTER backend resolution to ensure the
/// correct backend is captured (fixes the old thinking_middleware
/// mismatch where active_backend was used before routing decisions).
pub fn create_thinking(
    transformer_registry: &Arc<TransformerRegistry>,
    backend: &Backend,
    ctx: &mut PipelineContext,
) -> Option<ThinkingSession> {
    let session = transformer_registry.begin_request(
        &backend.name,
        ctx.debug_logger.clone(),
    );

    ctx.debug_logger.log_auxiliary(
        "thinking",
        None,
        None,
        Some(&format!(
            "Created thinking session for backend '{}'",
            backend.name
        )),
        None,
    );

    Some(session)
}
