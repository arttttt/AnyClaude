//! The proxy / config handles the GPU coordinator reads to render the chrome +
//! popups and writes on a backend switch / settings save. A plain bundle of the
//! live backend state, the subagent / teammate overrides, the observability hub,
//! and the settings manager — grouped so `GpuApp` isn't littered with five
//! separate handles. The coordinator's popup / chrome code reaches the fields
//! directly (they are `pub(super)`).

use crate::backend::{AgentBackendState, BackendState};
use crate::config::ClaudeSettingsManager;
use crate::metrics::ObservabilityHub;

pub(super) struct Backends {
    /// Live proxy backend state. The backend popup reads the list; Enter calls
    /// `switch_backend`; the history popup pulls `get_switch_log()`.
    pub(super) backend_state: BackendState,
    /// Subagent backend override (`None` = use the active backend).
    pub(super) subagent_backend: AgentBackendState,
    /// Teammate backend override (separate from the subagent one).
    pub(super) teammate_backend: AgentBackendState,
    /// Observability hub — the header reads the total request counter.
    pub(super) observability: ObservabilityHub,
    /// Settings registry + current values, persisted on the Cmd+E popup's Enter.
    pub(super) settings_manager: ClaudeSettingsManager,
}
