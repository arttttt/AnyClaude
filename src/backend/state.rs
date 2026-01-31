//! Backend management and hot-swap routing.
//!
//! Provides thread-safe backend state management with support for
//! runtime switching without interrupting in-flight requests.

use std::sync::{Arc, RwLock};
use std::time::SystemTime;

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
            active_backend,
            config,
            switch_log: Vec::new(),
        };

        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    /// Get the currently active backend ID.
    ///
    /// This is fast and non-blocking for concurrent readers.
    pub fn get_active_backend(&self) -> String {
        self.inner
            .read()
            .expect("backend state lock poisoned")
            .active_backend
            .clone()
    }

    /// Get the full configuration for the currently active backend.
    ///
    /// Returns an error if the backend is no longer in config (shouldn't happen
    /// unless config was reloaded with different backends).
    pub fn get_active_backend_config(&self) -> Result<Backend, BackendError> {
        let state = self.inner.read().expect("backend state lock poisoned");
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
        let state = self.inner.read().expect("backend state lock poisoned");
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
        self.inner
            .read()
            .expect("backend state lock poisoned")
            .config
            .clone()
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
        let mut state = self.inner.write().expect("backend state lock poisoned");

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
        tracing::info!(
            old_backend = %old_backend,
            new_backend = %backend_id,
            "Backend switched"
        );

        Ok(())
    }

    /// Get the switch log for debugging/auditing.
    pub fn get_switch_log(&self) -> Vec<SwitchLogEntry> {
        self.inner
            .read()
            .expect("backend state lock poisoned")
            .switch_log
            .clone()
    }

    /// Validate that a backend ID exists in the current configuration.
    pub fn validate_backend(&self, backend_id: &str) -> bool {
        let state = self.inner.read().expect("backend state lock poisoned");
        state.config.backends.iter().any(|b| b.name == backend_id)
    }

    /// Get list of available backend IDs.
    pub fn list_backends(&self) -> Vec<String> {
        let state = self.inner.read().expect("backend state lock poisoned");
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

        let mut state = self.inner.write().expect("backend state lock poisoned");

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

            tracing::warn!(
                old_backend = %state.active_backend,
                new_backend = %new_active,
                "Active backend no longer in config, switching to default"
            );

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

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> Config {
        Config {
            defaults: crate::config::Defaults {
                active: "backend1".to_string(),
                timeout_seconds: 30,
                connect_timeout_seconds: 5,
                idle_timeout_seconds: 60,
                pool_idle_timeout_seconds: 90,
                pool_max_idle_per_host: 8,
                max_retries: 3,
                retry_backoff_base_ms: 100,
            },
            proxy: crate::config::ProxyConfig::default(),
            backends: vec![
                Backend {
                    name: "backend1".to_string(),
                    display_name: "Backend 1".to_string(),
                    base_url: "https://api1.example.com".to_string(),
                    auth_type_str: "api_key".to_string(),
                    auth_env_var: "API1_KEY".to_string(),
                    api_key: None,
                    models: vec!["model1".to_string()],
                },
                Backend {
                    name: "backend2".to_string(),
                    display_name: "Backend 2".to_string(),
                    base_url: "https://api2.example.com".to_string(),
                    auth_type_str: "bearer".to_string(),
                    auth_env_var: "API2_KEY".to_string(),
                    api_key: None,
                    models: vec!["model2".to_string()],
                },
            ],
        }
    }

    #[test]
    fn test_from_config_with_default() {
        let config = create_test_config();
        let state = BackendState::from_config(config).unwrap();
        assert_eq!(state.get_active_backend(), "backend1");
    }

    #[test]
    fn test_from_config_no_default_uses_first() {
        let mut config = create_test_config();
        config.defaults.active = "".to_string();
        let state = BackendState::from_config(config).unwrap();
        assert_eq!(state.get_active_backend(), "backend1");
    }

    #[test]
    fn test_from_config_empty_backends_fails() {
        let mut config = create_test_config();
        config.backends.clear();
        assert!(matches!(
            BackendState::from_config(config),
            Err(BackendError::NoBackendsConfigured)
        ));
    }

    #[test]
    fn test_from_config_invalid_default_fails() {
        let mut config = create_test_config();
        config.defaults.active = "nonexistent".to_string();
        assert!(matches!(
            BackendState::from_config(config),
            Err(BackendError::BackendNotFound { .. })
        ));
    }

    #[test]
    fn test_switch_backend_success() {
        let config = create_test_config();
        let state = BackendState::from_config(config).unwrap();

        assert_eq!(state.get_active_backend(), "backend1");
        state.switch_backend("backend2").unwrap();
        assert_eq!(state.get_active_backend(), "backend2");
    }

    #[test]
    fn test_switch_backend_invalid_fails() {
        let config = create_test_config();
        let state = BackendState::from_config(config).unwrap();

        assert!(matches!(
            state.switch_backend("nonexistent"),
            Err(BackendError::BackendNotFound { .. })
        ));
        // State should be unchanged
        assert_eq!(state.get_active_backend(), "backend1");
    }

    #[test]
    fn test_switch_backend_same_noop() {
        let config = create_test_config();
        let state = BackendState::from_config(config).unwrap();

        state.switch_backend("backend1").unwrap();
        assert_eq!(state.get_active_backend(), "backend1");
        // Should not create a log entry for no-op switch
        assert!(state.get_switch_log().is_empty());
    }

    #[test]
    fn test_switch_log() {
        let config = create_test_config();
        let state = BackendState::from_config(config).unwrap();

        state.switch_backend("backend2").unwrap();
        state.switch_backend("backend1").unwrap();

        let log = state.get_switch_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].old_backend, Some("backend1".to_string()));
        assert_eq!(log[0].new_backend, "backend2".to_string());
        assert_eq!(log[1].old_backend, Some("backend2".to_string()));
        assert_eq!(log[1].new_backend, "backend1".to_string());
    }

    #[test]
    fn test_validate_backend() {
        let config = create_test_config();
        let state = BackendState::from_config(config).unwrap();

        assert!(state.validate_backend("backend1"));
        assert!(state.validate_backend("backend2"));
        assert!(!state.validate_backend("nonexistent"));
    }

    #[test]
    fn test_list_backends() {
        let config = create_test_config();
        let state = BackendState::from_config(config).unwrap();

        let backends = state.list_backends();
        assert_eq!(backends.len(), 2);
        assert!(backends.contains(&"backend1".to_string()));
        assert!(backends.contains(&"backend2".to_string()));
    }

    #[test]
    fn test_get_active_backend_config() {
        let config = create_test_config();
        let state = BackendState::from_config(config).unwrap();

        let backend = state.get_active_backend_config().unwrap();
        assert_eq!(backend.name, "backend1");
        assert_eq!(backend.base_url, "https://api1.example.com");
    }

    #[test]
    fn test_update_config() {
        let config = create_test_config();
        let state = BackendState::from_config(config.clone()).unwrap();

        // Switch to backend2
        state.switch_backend("backend2").unwrap();

        // Update config with new backend
        let mut new_config = config;
        new_config.backends.push(Backend {
            name: "backend3".to_string(),
            display_name: "Backend 3".to_string(),
            base_url: "https://api3.example.com".to_string(),
            auth_type_str: "api_key".to_string(),
            auth_env_var: "API3_KEY".to_string(),
            api_key: None,
            models: vec!["model3".to_string()],
        });

        state.update_config(new_config).unwrap();
        assert_eq!(state.get_active_backend(), "backend2"); // Should stay the same
        assert!(state.validate_backend("backend3"));
    }

    #[test]
    fn test_update_config_removes_active_backend() {
        let config = create_test_config();
        let state = BackendState::from_config(config.clone()).unwrap();

        // Switch to backend2
        state.switch_backend("backend2").unwrap();

        // Update config removing backend2
        let mut new_config = config;
        new_config.backends.retain(|b| b.name != "backend2");

        state.update_config(new_config).unwrap();
        // Should switch to default (backend1)
        assert_eq!(state.get_active_backend(), "backend1");
    }
}
