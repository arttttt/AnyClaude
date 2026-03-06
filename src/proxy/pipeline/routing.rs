//! Stage 2: Backend resolution.
//!
//! Resolves the target backend based on:
//! - Backend override from extensions (teammate pipeline)
//! - Plugin routing decisions
//! - AC marker in request body (session affinity from hook)
//! - Marker model prefixes (marker-*, anyclaude-*)
//! - Active backend from backend_state

use serde_json::Value;

use crate::backend::{BackendState, SubagentRegistry};
use crate::config::Backend;
use crate::metrics::{BackendOverride, RoutingDecision};
use crate::proxy::error::ProxyError;
use crate::proxy::pipeline::PipelineContext;

/// Stage 2: Resolve the target backend.
///
/// Priority:
/// 1. Plugin backend override (from observability.start_request)
/// 2. Explicit backend_override parameter (teammate routes)
/// 3. AC marker in request body (session affinity from hook)
/// 4. Marker model detection (marker-*, anyclaude-* prefixes, direct backend name)
/// 5. Active backend from backend_state
pub fn resolve_backend(
    backend_state: &BackendState,
    backend_override: Option<String>,
    plugin_override: Option<BackendOverride>,
    parsed_body: Option<&Value>,
    registry: &SubagentRegistry,
    ctx: &mut PipelineContext,
) -> Result<Backend, ProxyError> {
    // Resolve with documented priority.
    // Higher-priority overrides short-circuit — no body parsing needed.
    let (backend_id, routing_reason) = if let Some(ovr) = plugin_override {
        (ovr.backend, ovr.reason)
    } else if let Some(bo) = backend_override {
        (bo, "teammate route".into())
    } else if let Some(id) = parsed_body.and_then(extract_ac_marker) {
        let b = registry.lookup(&id).ok_or_else(|| {
            ProxyError::SubagentNotRegistered { id: id.clone() }
        })?;
        (b, "ac marker session affinity".into())
    } else if let Some(mb) = parsed_body
        .and_then(|body| body.get("model"))
        .and_then(|m| m.as_str())
        .and_then(|model| detect_marker_model(model, backend_state))
    {
        (mb, "marker model".into())
    } else {
        (backend_state.get_active_backend(), "active backend".into())
    };

    let backend = backend_state
        .get_backend_config(&backend_id)
        .map_err(|e| ProxyError::BackendNotFound {
            backend: e.to_string(),
        })?;

    ctx.span.set_backend(backend_id.clone());
    ctx.span.record_mut().routing_decision = Some(RoutingDecision {
        backend: backend_id,
        reason: routing_reason,
    });

    Ok(backend)
}

/// Extract `⟨AC:{id}⟩` marker from message content.
///
/// The marker is injected by the SubagentStart hook into the subagent's
/// context via `additionalContext`. It appears inside `messages[].content`
/// — either as a plain string or within a content block array.
pub fn extract_ac_marker(body: &Value) -> Option<String> {
    let messages = body.get("messages")?.as_array()?;
    for msg in messages {
        let Some(content) = msg.get("content") else { continue };
        match content {
            Value::String(s) => {
                if let Some(id) = parse_marker(s) {
                    return Some(id);
                }
            }
            Value::Array(blocks) => {
                for block in blocks {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        if let Some(id) = parse_marker(text) {
                            return Some(id);
                        }
                    }
                }
            }
            other => {
                crate::metrics::app_log(
                    "routing",
                    &format!("extract_ac_marker: unexpected content type: {}", other),
                );
            }
        }
    }
    None
}

/// Parse `⟨AC:{id}⟩` from a string slice.
fn parse_marker(s: &str) -> Option<String> {
    let start = s.find(SubagentRegistry::MARKER_PREFIX)?;
    let rest = &s[start + SubagentRegistry::MARKER_PREFIX.len()..];
    let end = rest.find(SubagentRegistry::MARKER_SUFFIX)?;
    let id = &rest[..end];
    if !id.is_empty() && id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        Some(id.to_string())
    } else {
        None
    }
}

/// Detect marker model and return corresponding backend.
///
/// Marker models are special model names that indicate the request
/// should be routed to a specific backend regardless of the active backend.
fn detect_marker_model(
    model: &str,
    backend_state: &BackendState,
) -> Option<String> {
    // Define marker patterns and their target backends
    // Format: "marker-{backend_name}" or "anyclaude-{backend_name}"
    let marker_prefixes = ["marker-", "anyclaude-"];

    for prefix in &marker_prefixes {
        if let Some(rest) = model.strip_prefix(prefix) {
            // Check if the rest is a valid backend
            if backend_state.validate_backend(rest) {
                crate::metrics::app_log(
                    "routing",
                    &format!("Detected marker model prefix '{}', routing to backend '{}'", prefix, rest),
                );
                return Some(rest.to_string());
            }
        }
    }

    // Also check for exact model name matching a backend
    // This allows routing by using the backend name as the model
    if backend_state.validate_backend(model) {
        crate::metrics::app_log(
            "routing",
            &format!("Model name matches backend '{}', using for routing", model),
        );
        return Some(model.to_string());
    }

    None
}
