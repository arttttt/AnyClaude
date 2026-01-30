use serde::{Deserialize, Serialize};

/// Root configuration container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub defaults: Defaults,
    pub backends: Vec<Backend>,
}

/// Default settings for the application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    /// Name of the active backend by default.
    pub active: String,
    /// Request timeout in seconds.
    pub timeout_seconds: u32,
    /// Connection timeout in seconds (default: 5).
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_seconds: u32,
    /// Idle timeout for streaming responses in seconds (default: 60).
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_seconds: u32,
    /// Pool idle timeout in seconds (default: 90).
    #[serde(default = "default_pool_idle_timeout")]
    pub pool_idle_timeout_seconds: u32,
    /// Max idle connections per host (default: 8).
    #[serde(default = "default_pool_max_idle_per_host")]
    pub pool_max_idle_per_host: u32,
    /// Max retry attempts for connection errors (default: 3).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Base backoff in milliseconds for retry (default: 100).
    #[serde(default = "default_retry_backoff_base_ms")]
    pub retry_backoff_base_ms: u64,
}

fn default_connect_timeout() -> u32 {
    5
}

fn default_idle_timeout() -> u32 {
    60
}

fn default_pool_idle_timeout() -> u32 {
    90
}

fn default_pool_max_idle_per_host() -> u32 {
    8
}

fn default_max_retries() -> u32 {
    3
}

fn default_retry_backoff_base_ms() -> u64 {
    100
}

/// Backend configuration for an API provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backend {
    /// Unique identifier (e.g., "claude", "glm", "openrouter").
    pub name: String,
    /// Display name in UI (e.g., "Claude", "GLM-4").
    pub display_name: String,
    /// Base URL for the API (e.g., "https://api.anthropic.com").
    pub base_url: String,
    /// Authentication type: "api_key", "bearer", "none".
    #[serde(rename = "auth_type")]
    pub auth_type_str: String,
    /// Environment variable name containing the key (e.g., "ANTHROPIC_API_KEY").
    pub auth_env_var: String,
    /// List of supported models.
    pub models: Vec<String>,
}

impl Default for Backend {
    fn default() -> Self {
        Self {
            name: "claude".to_string(),
            display_name: "Claude".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            auth_type_str: "api_key".to_string(),
            auth_env_var: "ANTHROPIC_API_KEY".to_string(),
            models: vec!["claude-sonnet-4-20250514".to_string()],
        }
    }
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            active: "claude".to_string(),
            timeout_seconds: 30,
            connect_timeout_seconds: 5,
            idle_timeout_seconds: 60,
            pool_idle_timeout_seconds: 90,
            pool_max_idle_per_host: 8,
            max_retries: 3,
            retry_backoff_base_ms: 100,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            defaults: Defaults::default(),
            backends: vec![Backend::default()],
        }
    }
}
