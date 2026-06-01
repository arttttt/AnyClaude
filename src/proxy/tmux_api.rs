//! `/api/tmux` — the tmux control plane endpoint (M3).
//!
//! The teammate tmux shim POSTs the tmux argv Claude Code issues (`{"args":[…]}`)
//! here; [`tmux_adapter::parse`](super::tmux_adapter::parse) reduces it to a
//! [`TmuxAction`], which this handler maps to a teammate lifecycle
//! [`ChildSessionEvent`](crate::ui::child_session::ChildSessionEvent) driven
//! through the winit coordinator via the [`ControlPlaneHandle`]. anyclaude is the
//! layout authority — there is no real tmux — so a verb registers / unregisters /
//! retitles a pane, and synchronous verbs (`split-window -P`) reply the pane's
//! tmux `%N`.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::proxy::tmux_adapter::{parse, TmuxAction};
use crate::ui::child_session::ChildSessionEvent;
use crate::ui::control_plane::ControlPlaneHandle;

/// Axum state for `/api/tmux` — only the bridge to the UI. Separate from
/// `HookState` so these handlers depend on nothing else (ISP).
#[derive(Clone)]
pub struct TmuxState {
    /// `None` when the proxy runs headless (no GPU UI) — handlers then 503.
    pub control_plane: Option<ControlPlaneHandle>,
}

/// The tmux invocation the shim forwards: the full argv (`args[0]` is the verb).
#[derive(Deserialize)]
pub struct TmuxRequest {
    pub args: Vec<String>,
}

/// POST /api/tmux
///
/// Parse the tmux argv and apply it. Lifecycle verbs cross to the coordinator;
/// `split-window` replies the minted `%N`; geometry/option verbs ack; queries
/// return empty (real data in a later step); an unknown verb is a 400 + log.
pub async fn handle_tmux(State(state): State<TmuxState>, Json(req): Json<TmuxRequest>) -> Response {
    match parse(&req.args) {
        TmuxAction::NewPane(spec) => {
            let Some(cp) = state.control_plane else {
                return no_ui();
            };
            match cp.submit(ChildSessionEvent::Register(spec)).await {
                Some(pane) => format!("%{}", pane.0).into_response(),
                None => (StatusCode::SERVICE_UNAVAILABLE, "UI did not reply").into_response(),
            }
        }
        TmuxAction::KillPane(pane) => {
            let Some(cp) = state.control_plane else {
                return no_ui();
            };
            cp.submit(ChildSessionEvent::Unregister(pane)).await;
            StatusCode::OK.into_response()
        }
        TmuxAction::SetTitle { pane, title } => {
            let Some(cp) = state.control_plane else {
                return no_ui();
            };
            cp.submit(ChildSessionEvent::SetTitle { pane, title }).await;
            StatusCode::OK.into_response()
        }
        TmuxAction::Ack => StatusCode::OK.into_response(),
        TmuxAction::Query(q) => {
            // Not answered with real data yet — log so a real CC run reveals the
            // expected format, return empty (CC tolerates an empty query result
            // better than a 5xx). Real registry-backed queries land in a later step.
            crate::metrics::app_log("tmux", &format!("unanswered query: {q}"));
            StatusCode::OK.into_response()
        }
        TmuxAction::Unknown(verb) => {
            crate::metrics::app_log("tmux", &format!("unknown verb: {verb}"));
            (StatusCode::BAD_REQUEST, format!("unknown tmux verb: {verb}")).into_response()
        }
    }
}

/// 503 when the proxy is headless (no UI to drive).
fn no_ui() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, "no UI attached").into_response()
}
