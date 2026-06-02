//! `ControlPlaneHandle` — the tokio→winit bridge for the teammate control plane.
//!
//! The proxy's tmux adapter runs on a tokio worker; the [`ChildSessionManager`](
//! super::child_session::ChildSessionManager) lives on the winit thread. A
//! lifecycle [`ChildSessionEvent`](super::child_session::ChildSessionEvent)
//! crosses the boundary through this handle: the adapter submits an event and
//! `.await`s the resulting [`PaneId`](super::child_session::PaneId) (the tmux
//! `%N`) for synchronous verbs like `split-window -P`.
//!
//! The handle deliberately does NOT name the renderer's `UserEvent` type — it
//! wraps an opaque submit closure built on the UI side (where it constructs the
//! `UserEvent` and wakes the loop via the `EventLoopProxy`). So the proxy
//! depends only on this small async API, not on the GPU UI (SRP / decoupling).

use std::sync::Arc;

use tokio::sync::oneshot;

use super::child_session::{ChildSessionEvent, PaneId};

/// One control-plane request crossing tokio→winit: a lifecycle event plus the
/// reply channel the coordinator answers with the resulting `PaneId`.
#[derive(Debug)]
pub struct ControlRequest {
    pub event: ChildSessionEvent,
    /// Answered with `Some(PaneId)` for a `Register`, `None` otherwise.
    pub reply: oneshot::Sender<Option<PaneId>>,
}

/// Async handle the proxy uses to drive the winit coordinator. Cloneable and
/// `Send`/`Sync`, so every axum handler can hold one. Its single job is to ship
/// a [`ControlRequest`] to the UI thread and await the reply.
#[derive(Clone)]
pub struct ControlPlaneHandle {
    submit: Arc<dyn Fn(ControlRequest) + Send + Sync>,
}

impl ControlPlaneHandle {
    /// Build a handle from the UI-side submit closure (which constructs the
    /// `UserEvent` and sends it through the `EventLoopProxy`).
    pub fn new(submit: impl Fn(ControlRequest) + Send + Sync + 'static) -> Self {
        Self { submit: Arc::new(submit) }
    }

    /// Submit `event` to the coordinator and await the resulting `PaneId`
    /// (`Some` for a `Register`, `None` otherwise — also `None` if the UI has
    /// dropped the reply, e.g. during shutdown).
    pub async fn submit(&self, event: ChildSessionEvent) -> Option<PaneId> {
        let (reply, rx) = oneshot::channel();
        (self.submit)(ControlRequest { event, reply });
        rx.await.ok().flatten()
    }
}
