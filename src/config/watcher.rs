//! Config hot-reload with file watching and debouncing.
//!
//! Provides thread-safe config access and automatic reload on file changes.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;

use crate::config::loader::ConfigError;
use crate::config::types::Config;
use crate::ui::events::AppEvent;

/// Errors that can occur during config watching.
#[derive(Debug, Error)]
pub enum WatcherError {
    #[error("Failed to create file watcher: {0}")]
    WatcherInit(#[from] notify::Error),

    #[error("Config path has no parent directory")]
    NoParentDir,
}

/// Thread-safe config container with interior mutability.
///
/// Allows multiple readers to access config concurrently while
/// supporting atomic updates during reload.
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
        self.inner.read().expect("config lock poisoned").clone()
    }

    /// Reload config from the file.
    ///
    /// On success, atomically replaces the current config.
    /// On failure, keeps the old config and returns the error.
    pub fn reload(&self) -> Result<(), ConfigError> {
        let config = Config::load_from(&self.path)?;
        let mut guard = self.inner.write().expect("config lock poisoned");
        *guard = config;
        Ok(())
    }

    /// Get the path being watched.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Watches the config file and triggers reloads on changes.
///
/// Runs in a background thread with debouncing to group rapid edits.
/// Sends `AppEvent::ConfigReload` on successful reload.
pub struct ConfigWatcher {
    // The watcher is kept alive by being stored here.
    // When ConfigWatcher is dropped, the watcher thread stops.
    _watcher: RecommendedWatcher,
    // Handle to the debounce thread for cleanup
    _debounce_handle: thread::JoinHandle<()>,
}

impl ConfigWatcher {
    /// Start watching the config file.
    ///
    /// # Arguments
    /// * `store` - The ConfigStore to reload
    /// * `event_tx` - Channel to send reload events to the UI
    /// * `debounce_ms` - Debounce delay in milliseconds (typically 200)
    ///
    /// # Errors
    /// Returns error if the watcher cannot be initialized or the path is invalid.
    pub fn start(
        store: ConfigStore,
        event_tx: mpsc::Sender<AppEvent>,
        debounce_ms: u64,
    ) -> Result<Self, WatcherError> {
        let config_path = store.path().to_path_buf();
        let watch_dir = config_path.parent().ok_or(WatcherError::NoParentDir)?;
        let config_filename = config_path
            .file_name()
            .map(|s| s.to_os_string())
            .unwrap_or_default();

        // Channel for raw file events
        let (raw_tx, raw_rx) = mpsc::channel();

        // Create the file watcher
        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                if let Ok(event) = result {
                    let _ = raw_tx.send(event);
                }
            },
            notify::Config::default(),
        )?;

        // Watch the parent directory (handles file deletion + recreation)
        watcher.watch(watch_dir, RecursiveMode::NonRecursive)?;

        // Spawn debounce thread
        let debounce_handle = thread::spawn(move || {
            debounce_loop(raw_rx, store, event_tx, config_filename, debounce_ms);
        });

        Ok(Self {
            _watcher: watcher,
            _debounce_handle: debounce_handle,
        })
    }
}

/// Debounce loop that groups rapid file changes.
///
/// Waits for `debounce_ms` after the last event before triggering reload.
fn debounce_loop(
    rx: mpsc::Receiver<Event>,
    store: ConfigStore,
    event_tx: mpsc::Sender<AppEvent>,
    config_filename: std::ffi::OsString,
    debounce_ms: u64,
) {
    let debounce = Duration::from_millis(debounce_ms);
    let mut pending_reload: Option<Instant> = None;

    loop {
        let timeout = if pending_reload.is_some() {
            debounce
        } else {
            // Long timeout when no pending reload
            Duration::from_secs(60)
        };

        match rx.recv_timeout(timeout) {
            Ok(event) => {
                // Check if this event affects our config file
                if is_config_event(&event, &config_filename) {
                    pending_reload = Some(Instant::now());
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // Check if debounce period has passed
                if let Some(last) = pending_reload {
                    if last.elapsed() >= debounce {
                        // Time to reload
                        match store.reload() {
                            Ok(()) => {
                                // Notify UI of successful reload
                                let _ = event_tx.send(AppEvent::ConfigReload);
                            }
                            Err(e) => {
                                // Log error but keep old config
                                eprintln!("Config reload failed: {}", e);
                            }
                        }
                        pending_reload = None;
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                // Watcher was dropped, exit the loop
                break;
            }
        }
    }
}

/// Check if a notify event affects the config file.
fn is_config_event(event: &Event, config_filename: &std::ffi::OsString) -> bool {
    // Only care about modifications and creates
    let dominated = matches!(
        event.kind,
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
    );

    if !dominated {
        return false;
    }

    // Check if any of the affected paths match our config file
    event.paths.iter().any(|p| {
        p.file_name()
            .map(|name| name == config_filename)
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_config(dir: &Path) -> PathBuf {
        let config_path = dir.join("config.toml");
        let content = r#"
[defaults]
active = "test"
timeout_seconds = 30

[[backends]]
name = "test"
display_name = "Test"
base_url = "https://test.example.com"
auth_type = "api_key"
api_key = "test-key-value"
models = ["model-1"]
"#;
        fs::write(&config_path, content).unwrap();
        config_path
    }

    #[test]
    fn test_config_store_get() {
        let config = Config::default();
        let store = ConfigStore::new(config.clone(), PathBuf::from("/test/config.toml"));
        let retrieved = store.get();
        assert_eq!(retrieved.defaults.active, config.defaults.active);
    }

    #[test]
    fn test_config_store_reload() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(temp_dir.path());

        let initial_config = Config::default();
        let store = ConfigStore::new(initial_config, config_path.clone());

        // Reload should succeed and update the config
        store.reload().unwrap();
        let reloaded = store.get();
        assert_eq!(reloaded.defaults.active, "test");
    }

    #[test]
    fn test_config_store_reload_invalid() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        fs::write(&config_path, "invalid { toml }").unwrap();

        let initial_config = Config::default();
        let store = ConfigStore::new(initial_config.clone(), config_path);

        // Reload should fail
        assert!(store.reload().is_err());

        // Original config should be preserved
        let current = store.get();
        assert_eq!(current.defaults.active, initial_config.defaults.active);
    }
}
