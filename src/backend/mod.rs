//! Backend management and hot-swap routing.
//!
//! Provides thread-safe backend state management with support for
//! runtime switching without interrupting in-flight requests.

mod state;

pub use state::{BackendError, BackendState, SubagentBackend, SwitchLogEntry};

/// Manager for backend operations (placeholder for future CRUD operations).
///
/// Currently, backend configuration is handled through the config module.
/// This manager will be expanded for health checks and dynamic backend
/// registration in future iterations.
pub struct BackendManager {
    state: BackendState,
}

impl BackendManager {
    /// Create a new BackendManager from a BackendState.
    pub fn new(state: BackendState) -> Self {
        Self { state }
    }

    /// Get a reference to the backend state.
    pub fn state(&self) -> &BackendState {
        &self.state
    }

    /// Get a clone of the backend state.
    pub fn state_clone(&self) -> BackendState {
        self.state.clone()
    }
}
