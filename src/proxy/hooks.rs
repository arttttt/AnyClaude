//! Hook handlers for CC SubagentStart / SubagentStop events.
//!
//! CC fires these hooks when it spawns or stops in-process subagents.
//! We use SubagentStart to register the subagent's identifier in the
//! registry, mapping it to the backend that was active at spawn time.
//! The identifier is injected into `additionalContext` as `⟨AC:{id}⟩`,
//! and resolved back to a backend at routing time via registry lookup.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::backend::{SubagentBackend, SubagentRegistry};

/// Axum state for hook endpoints — bundles the two pieces of state
/// that hook handlers need.
#[derive(Clone)]
pub struct HookState {
    pub subagent_backend: SubagentBackend,
    pub registry: SubagentRegistry,
}

/// Input sent by CC hook (piped via curl stdin).
///
/// CC sends various fields — we only deserialize what we use.
/// Unknown fields are silently ignored by serde.
#[derive(Deserialize)]
pub struct SubagentHookInput {
    /// Unique identifier for this subagent instance (e.g. "a1b2c3d4e5f6a7b8").
    /// Used as the registry key for session affinity.
    pub agent_id: Option<String>,
    /// Parent session ID. Not used for routing but kept for diagnostics.
    pub session_id: Option<String>,
}

#[derive(Serialize)]
pub struct HookSpecificOutput {
    #[serde(rename = "hookEventName")]
    pub hook_event_name: String,
    #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
}

#[derive(Serialize)]
pub struct SubagentStartResponse {
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: HookSpecificOutput,
}

/// POST /api/subagent-start
///
/// Registers the subagent's identifier in the registry, mapping it to
/// the currently active subagent backend. Injects `⟨AC:{id}⟩` into
/// `additionalContext` so routing can look it up later.
pub async fn handle_subagent_start(
    State(state): State<HookState>,
    Json(input): Json<SubagentHookInput>,
) -> Json<SubagentStartResponse> {
    let context = match (input.agent_id.as_deref(), state.subagent_backend.get()) {
        (Some(id), Some(backend)) => {
            state.registry.register(id, &backend);
            crate::metrics::app_log(
                "hooks",
                &format!("SubagentStart: registered '{}' → backend '{}'", id, backend),
            );
            Some(SubagentRegistry::format_marker(id))
        }
        _ => {
            crate::metrics::app_log(
                "hooks",
                "SubagentStart: no agent_id or no subagent backend configured",
            );
            None
        }
    };

    Json(SubagentStartResponse {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "SubagentStart".into(),
            additional_context: context,
        },
    })
}

/// POST /api/subagent-stop
///
/// Removes the subagent's identifier from the registry.
pub async fn handle_subagent_stop(
    State(state): State<HookState>,
    Json(input): Json<SubagentHookInput>,
) -> axum::http::StatusCode {
    if let Some(id) = input.agent_id.as_deref() {
        state.registry.remove(id);
        crate::metrics::app_log("hooks", &format!("SubagentStop: removed '{}'", id));
    }
    axum::http::StatusCode::OK
}
