//! Thread-safe configuration storage.
//!
//! Provides a simple in-memory config container with interior mutability.
//! Config file watching has been removed to avoid race conditions with
//! CLI argument overrides.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::config::loader::ConfigError;
use crate::config::types::Config;

/// Thread-safe config container with interior mutability.
///
/// Allows multiple readers to access config concurrently while
/// supporting atomic updates when needed.
#[derive(Clone)]
pub struct ConfigStore {
    inner: Arc<RwLock<Config>>,
    path: PathBuf,
}

impl ConfigStore {
    /// Create a new ConfigStore from initial config and path.
    pub fn new(config: Config, path: PathBuf) -> Self {
        Self {
            inner: Arc::new(RwLock::new(config)),
            path,
        }
    }

    /// Get a clone of the current config.
    ///
    /// This is cheap because Config is Clone.
    /// Multiple readers can call this concurrently.
    pub fn get(&self) -> Config {
        self.inner.read().clone()
    }

    /// Reload config from the file.
    ///
    /// On success, atomically replaces the current config.
    /// On failure, keeps the old config and returns the error.
    pub fn reload(&self) -> Result<(), ConfigError> {
        let config = Config::load_from(&self.path)?;
        let mut guard = self.inner.write();
        *guard = config;
        Ok(())
    }

    /// Get the config file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}
