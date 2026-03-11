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
    } else if let Some(id) = (!registry.is_empty())
        .then(|| parsed_body.and_then(extract_ac_marker))
        .flatten()
    {
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

/// CC wrapper prefix that always precedes our marker in `additionalContext`.
/// Without this prefix the marker is ignored — prevents false positives
/// from user text that happens to contain `⟨AC:...⟩`.
const HOOK_CONTEXT_PREFIX: &str = "SubagentStart hook additional context:";

/// Max user messages to scan for the marker.
/// Hook context is prepended at the start, so scanning beyond the first
/// few messages is wasteful and increases false-positive surface.
const MAX_MESSAGES_TO_SCAN: usize = 3;

/// Extract `⟨AC:{id}⟩` marker from the first few user messages.
///
/// The marker is injected by the SubagentStart hook via `additionalContext`.
/// CC wraps it in `<system-reminder>SubagentStart hook additional context: …</system-reminder>`
/// and places it as an early user message.
///
/// Multi-layer protection against false positives:
/// 1. Registry guard in `resolve_backend` — skips entirely when no subagents exist
/// 2. Only first [`MAX_MESSAGES_TO_SCAN`] messages are checked
/// 3. Only `role: "user"` messages are checked
/// 4. The CC-generated [`HOOK_CONTEXT_PREFIX`] must precede the marker
/// 5. Registry lookup must succeed for the extracted id
pub fn extract_ac_marker(body: &Value) -> Option<String> {
    let messages = body.get("messages")?.as_array()?;

    for msg in messages.iter().take(MAX_MESSAGES_TO_SCAN) {
        // Only user messages — hook context is always injected as role: "user"
        if msg.get("role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }

        let Some(content) = msg.get("content") else { continue };
        match content {
            Value::String(s) => {
                if let Some(id) = parse_marker_in_hook_context(s) {
                    return Some(id);
                }
            }
            Value::Array(blocks) => {
                for block in blocks {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        if let Some(id) = parse_marker_in_hook_context(text) {
                            return Some(id);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse `⟨AC:{id}⟩` only when preceded by the CC hook context prefix.
///
/// Rejects markers that appear in arbitrary user text — the full
/// `SubagentStart hook additional context: ⟨AC:…⟩` sequence is required.
fn parse_marker_in_hook_context(s: &str) -> Option<String> {
    let prefix_start = s.find(HOOK_CONTEXT_PREFIX)?;
    let after_prefix = &s[prefix_start + HOOK_CONTEXT_PREFIX.len()..];
    parse_marker(after_prefix)
}

/// Parse `⟨AC:{id}⟩` from a string slice.
fn parse_marker(s: &str) -> Option<String> {
    let start = s.find(SubagentRegistry::MARKER_PREFIX)?;
    let rest = &s[start + SubagentRegistry::MARKER_PREFIX.len()..];
    let end = rest.find(SubagentRegistry::MARKER_SUFFIX)?;
    let id = &rest[..end];
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-' || c == '_') {
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
