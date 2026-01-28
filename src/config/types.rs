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
}

fn default_connect_timeout() -> u32 {
    5
}

fn default_idle_timeout() -> u32 {
    60
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
