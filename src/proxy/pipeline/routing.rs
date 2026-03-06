//! Stage 2: Backend resolution.
//!
//! Resolves the target backend based on:
//! - Backend override from extensions (teammate pipeline)
//! - Plugin routing decisions
//! - AC marker in request body (session affinity from hook)
//! - Marker model prefixes (marker-*, anyclaude-*)
//! - Active backend from backend_state

use serde_json::Value;

use crate::backend::BackendState;
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
    ctx: &mut PipelineContext,
) -> Result<Backend, ProxyError> {
    // Get the active backend for reference
    let active_backend = backend_state.get_active_backend();

    // Extract AC marker from request body (session affinity from hook)
    let ac_marker_backend = parsed_body.and_then(extract_ac_marker);

    // Check for marker model in request body
    let marker_backend = parsed_body
        .and_then(|body| body.get("model"))
        .and_then(|m| m.as_str())
        .and_then(|model| detect_marker_model(model, backend_state));

    // Determine final backend ID with priority:
    // plugin_override > backend_override (teammate) > ac_marker_backend > marker_backend > active_backend
    let backend_id = plugin_override
        .as_ref()
        .map(|o| o.backend.clone())
        .or(backend_override.clone())
        .or(ac_marker_backend.clone())
        .or(marker_backend.clone())
        .unwrap_or(active_backend);

    // Resolve backend configuration
    let backend = backend_state
        .get_backend_config(&backend_id)
        .map_err(|e| ProxyError::BackendNotFound {
            backend: e.to_string(),
        })?;

    // Record routing decision
    let routing_reason = if plugin_override.is_some() {
        plugin_override.map(|o| o.reason).unwrap_or_else(|| "plugin".to_string())
    } else if backend_override.is_some() {
        "teammate route".to_string()
    } else if ac_marker_backend.is_some() {
        "ac marker session affinity".to_string()
    } else if marker_backend.is_some() {
        "marker model".to_string()
    } else {
        "active backend".to_string()
    };

    ctx.span.set_backend(backend_id.clone());
    ctx.span.record_mut().routing_decision = Some(RoutingDecision {
        backend: backend_id,
        reason: routing_reason,
    });

    Ok(backend)
}

/// Extract `⟨AC:backend_name⟩` marker from request body.
///
/// The marker is injected by the SubagentStart hook into the subagent's
/// context via `additionalContext`. It appears as a `<system-reminder>`
/// in the message stream, so we search the serialized body.
pub fn extract_ac_marker(body: &Value) -> Option<String> {
    let body_str = body.to_string();
    let marker_start = "\u{27E8}AC:";
    let marker_end = '\u{27E9}';
    let start = body_str.find(marker_start)?;
    let rest = &body_str[start + marker_start.len()..];
    let end = rest.find(marker_end)?;
    let backend = &rest[..end];
    if !backend.is_empty()
        && backend
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        Some(backend.to_string())
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
