//! Configuration management for claudewrapper.
//!
//! This module handles loading, parsing, and validating configuration
//! from TOML files, as well as resolving API credentials from environment
//! variables.

mod auth;
mod credentials;
mod loader;
mod types;
mod watcher;

pub use auth::{build_auth_header, AuthHeader};
pub use credentials::{AuthType, CredentialStatus, SecureString};
pub use loader::ConfigError;
pub use types::{Backend, Config, Defaults, ProxyConfig};
pub use watcher::{ConfigStore, ConfigWatcher, WatcherError};
