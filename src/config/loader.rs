use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::config::credentials::CredentialStatus;
use crate::config::types::{Backend, Config};

/// Errors that can occur when loading configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read config file '{path}': {source}")]
    ReadError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse config file '{path}': {source}")]
    ParseError {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("Config validation failed: {message}")]
    ValidationError { message: String },
}

impl Config {
    /// Returns the path to the configuration file.
    ///
    /// Uses `~/.config/claude-wrapper/config.toml` on Unix/macOS,
    /// or equivalent on other platforms via `dirs::config_dir()`.
    /// Falls back to current directory if config_dir is unavailable.
    pub fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        config_dir.join("claude-wrapper").join("config.toml")
    }

    /// Loads configuration from the default config file.
    ///
    /// - If the file doesn't exist, returns `Config::default()`.
    /// - If the file exists, parses it as TOML and validates.
    /// - Returns an error if reading, parsing, or validation fails.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&Self::config_path())
    }

    /// Loads configuration from a specific path.
    ///
    /// - If the file doesn't exist, returns `Config::default()`.
    /// - If the file exists, parses it as TOML and validates.
    /// - Returns an error if reading, parsing, or validation fails.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Config::default());
        }

        let content = fs::read_to_string(path).map_err(|e| ConfigError::ReadError {
            path: path.to_path_buf(),
            source: e,
        })?;

        let config: Config = toml::from_str(&content).map_err(|e| ConfigError::ParseError {
            path: path.to_path_buf(),
            source: e,
        })?;

        config.validate()?;
        Ok(config)
    }

    /// Validates the configuration.
    ///
    /// Checks:
    /// - At least one backend is configured
    /// - The active backend exists in the backends list
    /// - The active backend has valid credentials (or doesn't require them)
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.backends.is_empty() {
            return Err(ConfigError::ValidationError {
                message: "At least one backend must be configured".to_string(),
            });
        }

        let active = &self.defaults.active;
        let active_backend = self.backends.iter().find(|b| &b.name == active);

        match active_backend {
            None => {
                return Err(ConfigError::ValidationError {
                    message: format!(
                        "Active backend '{}' not found in configured backends",
                        active
                    ),
                });
            }
            Some(backend) => {
                if !backend.is_configured() {
                    return Err(ConfigError::ValidationError {
                        message: format!(
                            "Active backend '{}' is not configured - set api_key in config",
                            backend.name
                        ),
                    });
                }
            }
        }

        Ok(())
    }

    /// Log the status of all backends at startup.
    ///
    /// Logs warnings for unconfigured backends and info for configured ones.
    /// Never logs actual API key values.
    pub fn log_backend_status(&self) {
        for backend in &self.backends {
            match backend.resolve_credential() {
                CredentialStatus::Unconfigured { reason } => {
                    eprintln!(
                        "Warning: Backend '{}' is unconfigured - {}",
                        backend.name, reason
                    );
                }
                CredentialStatus::Configured(_) => {
                    // Don't log key value - just confirmation
                    eprintln!("Backend '{}' configured", backend.name);
                }
                CredentialStatus::NoAuth => {
                    eprintln!("Backend '{}' configured (no auth required)", backend.name);
                }
            }
        }
    }

    /// Get only backends that are configured (have valid credentials or don't need them).
    pub fn configured_backends(&self) -> Vec<&Backend> {
        self.backends.iter().filter(|b| b.is_configured()).collect()
    }

    /// Get the currently active backend, if configured.
    pub fn active_backend(&self) -> Option<&Backend> {
        self.backends
            .iter()
            .find(|b| b.name == self.defaults.active && b.is_configured())
    }
}
