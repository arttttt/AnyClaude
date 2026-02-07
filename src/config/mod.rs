//! Configuration management for anyclaude.
//!
//! This module handles loading, parsing, and validating configuration
//! from TOML files, as well as resolving API credentials from environment
//! variables.

mod auth;
mod credentials;
mod loader;
mod store;
mod types;

pub use auth::{build_auth_header, AuthHeader};
pub use credentials::{AuthType, CredentialStatus, SecureString};
pub use loader::ConfigError;
pub use store::ConfigStore;
pub use types::{
    Backend, BackendPricing, Config, DebugLogDestination, DebugLogFormat, DebugLogLevel,
    DebugLogRotation, DebugLogRotationMode, DebugLoggingConfig, Defaults, ProxyConfig,
    TerminalConfig,
};
