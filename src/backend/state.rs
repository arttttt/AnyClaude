//! Backend management and hot-swap routing.
//!
//! Provides thread-safe backend state management with support for
//! runtime switching without interrupting in-flight requests.

use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::RwLock;

use crate::config::{Backend, Config};

/// Errors that can occur during backend operations.
#[derive(Debug, Clone)]
pub enum BackendError {
    /// The requested backend does not exist in configuration.
    BackendNotFound { backend: String },
    /// No backends are configured.
    NoBackendsConfigured,
    /// The backend is not properly configured (e.g., missing env var).
    BackendNotConfigured { backend: String, reason: String },
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::BackendNotFound { backend } => {
                write!(f, "Backend '{}' not found", backend)
            }
            BackendError::NoBackendsConfigured => {
                write!(f, "No backends configured")
            }
            BackendError::BackendNotConfigured { backend, reason } => {
                write!(f, "Backend '{}' not configured: {}", backend, reason)
            }
        }
    }
}

impl std::error::Error for BackendError {}

/// Log entry for a backend switch event.
#[derive(Debug, Clone)]
pub struct SwitchLogEntry {
    /// When the switch occurred.
    pub timestamp: SystemTime,
    /// The previous active backend (None if initial state).
    pub old_backend: Option<String>,
    /// The new active backend.
    pub new_backend: String,
}

/// Thread-safe backend state with hot-swap support.
///
/// Uses a read-write lock pattern: many concurrent readers (requests)
/// can read the active backend, while writes (switches) are exclusive.
#[derive(Clone)]
pub struct BackendState {
    inner: Arc<RwLock<BackendStateInner>>,
}

struct BackendStateInner {
    /// The currently active backend ID.
    active_backend: String,
    /// Full configuration (needed to look up backend details).
    config: Config,
    /// History of backend switches for debugging/auditing.
    switch_log: Vec<SwitchLogEntry>,
}

impl BackendState {
    /// Create a new BackendState from configuration.
    ///
    /// # Errors
    /// Returns error if no backends are configured or if the default
    /// backend specified in config doesn't exist.
    pub fn from_config(config: Config) -> Result<Self, BackendError> {
        if config.backends.is_empty() {
            return Err(BackendError::NoBackendsConfigured);
        }

        // Determine initial active backend
        let active_backend = if config.defaults.active.is_empty() {
            // Use first backend if no default specified
            config.backends[0].name.clone()
        } else {
            // Validate the default backend exists
            let default = &config.defaults.active;
            if !config.backends.iter().any(|b| &b.name == default) {
                return Err(BackendError::BackendNotFound {
                    backend: default.clone(),
                });
            }
            default.clone()
        };

        let inner = BackendStateInner {
            active_backend: active_backend.clone(),
            config,
            switch_log: vec![SwitchLogEntry {
                timestamp: SystemTime::now(),
                old_backend: None,
                new_backend: active_backend,
            }],
        };

        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    /// Get the currently active backend ID.
    ///
    /// This is fast and non-blocking for concurrent readers.
    pub fn get_active_backend(&self) -> String {
        self.inner.read().active_backend.clone()
    }

    /// Get the full configuration for the currently active backend.
    ///
    /// Returns an error if the backend is no longer in config (shouldn't happen
    /// unless config was reloaded with different backends).
    pub fn get_active_backend_config(&self) -> Result<Backend, BackendError> {
        let state = self.inner.read();
        state
            .config
            .backends
            .iter()
            .find(|b| b.name == state.active_backend)
            .cloned()
            .ok_or_else(|| BackendError::BackendNotFound {
                backend: state.active_backend.clone(),
            })
    }

    /// Get the configuration for a specific backend by name.
    pub fn get_backend_config(&self, backend_id: &str) -> Result<Backend, BackendError> {
        let state = self.inner.read();
        state
            .config
            .backends
            .iter()
            .find(|b| b.name == backend_id)
            .cloned()
            .ok_or_else(|| BackendError::BackendNotFound {
                backend: backend_id.to_string(),
            })
    }

    /// Get the full current configuration.
    pub fn get_config(&self) -> Config {
        self.inner.read().config.clone()
    }

    /// Get config and active backend atomically under a single lock.
    pub fn get_config_and_active_backend(&self) -> (Config, String) {
        let state = self.inner.read();
        (state.config.clone(), state.active_backend.clone())
    }

    /// Switch to a different backend.
    ///
    /// # Arguments
    /// * `backend_id` - The ID of the backend to switch to
    ///
    /// # Errors
    /// Returns error if the backend doesn't exist. State is unchanged on error.
    ///
    /// # Performance
    /// Switch is atomic and takes less than 1ms under normal conditions.
    pub fn switch_backend(&self, backend_id: &str) -> Result<(), BackendError> {
        let mut state = self.inner.write();

        // Validate the target backend exists
        if !state.config.backends.iter().any(|b| b.name == backend_id) {
            return Err(BackendError::BackendNotFound {
                backend: backend_id.to_string(),
            });
        }

        // Don't switch if already active
        if state.active_backend == backend_id {
            return Ok(());
        }

        // Log the switch
        let entry = SwitchLogEntry {
            timestamp: SystemTime::now(),
            old_backend: Some(state.active_backend.clone()),
            new_backend: backend_id.to_string(),
        };
        state.switch_log.push(entry);

        // Perform the atomic switch
        let old_backend = state.active_backend.clone();
        state.active_backend = backend_id.to_string();

        // Log at info level for visibility
        crate::metrics::app_log("backend", &format!("Backend switched: {} -> {}", old_backend, backend_id));

        Ok(())
    }

    /// Get the switch log for debugging/auditing.
    pub fn get_switch_log(&self) -> Vec<SwitchLogEntry> {
        self.inner.read().switch_log.clone()
    }

    /// Validate that a backend ID exists in the current configuration.
    pub fn validate_backend(&self, backend_id: &str) -> bool {
        let state = self.inner.read();
        state.config.backends.iter().any(|b| b.name == backend_id)
    }

    /// Get list of available backend IDs.
    pub fn list_backends(&self) -> Vec<String> {
        let state = self.inner.read();
        state
            .config
            .backends
            .iter()
            .map(|b| b.name.clone())
            .collect()
    }

    /// Update the configuration (used when config file is reloaded).
    ///
    /// If the current active backend no longer exists in the new config,
    /// it will be switched to the default or first available backend.
    pub fn update_config(&self, new_config: Config) -> Result<(), BackendError> {
        if new_config.backends.is_empty() {
            return Err(BackendError::NoBackendsConfigured);
        }

        let mut state = self.inner.write();

        // Check if current backend still exists
        let current_exists = new_config
            .backends
            .iter()
            .any(|b| b.name == state.active_backend);

        if !current_exists {
            // Switch to default or first available
            let new_active = if new_config.defaults.active.is_empty() {
                new_config.backends[0].name.clone()
            } else {
                new_config.defaults.active.clone()
            };

            crate::metrics::app_log("backend", &format!("Active backend {} no longer in config, switching to {}", state.active_backend, new_active));

            let entry = SwitchLogEntry {
                timestamp: SystemTime::now(),
                old_backend: Some(state.active_backend.clone()),
                new_backend: new_active.clone(),
            };
            state.switch_log.push(entry);
            state.active_backend = new_active;
        }

        state.config = new_config;
        Ok(())
    }
}
