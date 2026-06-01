//! `/api/tmux/*` — the tmux control plane (M3).
//!
//! The teammate tmux shim POSTs the tmux verbs Claude Code issues here; each is
//! translated into a teammate lifecycle [`ChildSessionEvent`](
//! crate::ui::child_session::ChildSessionEvent) and driven through the winit
//! coordinator via the [`ControlPlaneHandle`]. anyclaude is the layout authority
//! — it doesn't run a real tmux — so a verb's job is to register / unregister /
//! retitle a pane, and synchronous verbs (`split-window -P`) reply the pane's
//! tmux `%N`.
//!
//! C1 wires the boundary with a single smoke handler (`split-window`); the full
//! verb set (`kill-pane`, `select-pane`, `send-keys`, queries) lands in C2/C3.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::ui::child_session::{ChildSessionEvent, ChildSpec};
use crate::ui::control_plane::ControlPlaneHandle;

/// Axum state for the `/api/tmux/*` routes — only the bridge to the UI. Kept
/// separate from `HookState` so these handlers depend on nothing else (ISP).
#[derive(Clone)]
pub struct TmuxState {
    /// `None` when the proxy runs headless (no GPU UI) — handlers then 503.
    pub control_plane: Option<ControlPlaneHandle>,
}

/// Default accent for a teammate pane until `select-pane -P` sets its colour.
const DEFAULT_ACCENT: [f32; 4] = [0.30, 0.55, 0.95, 1.0];

/// POST /api/tmux/split-window
///
/// C1 smoke handler: register a placeholder teammate (a live shell) through the
/// control plane and reply its tmux `%N`. C3 replaces the placeholder spec with
/// the real `claude …` command parsed from the following `send-keys`.
pub async fn handle_split_window(State(state): State<TmuxState>) -> Response {
    let Some(cp) = state.control_plane else {
        // Headless proxy (no UI attached) — nothing to drive.
        return (StatusCode::SERVICE_UNAVAILABLE, "no UI attached").into_response();
    };
    let spec = ChildSpec {
        name: "teammate".to_string(),
        accent: DEFAULT_ACCENT,
        command: std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
        args: vec![],
        env: vec![("TERM".to_string(), "xterm-256color".to_string())],
    };
    match cp.submit(ChildSessionEvent::Register(spec)).await {
        Some(pane) => format!("%{}", pane.0).into_response(),
        None => (StatusCode::SERVICE_UNAVAILABLE, "UI did not reply").into_response(),
    }
}
