use serde::{Deserialize, Serialize};

/// Root configuration container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub defaults: Defaults,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub thinking: ThinkingConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub debug_logging: DebugLoggingConfig,
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

/// Proxy configuration for local routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Bind address for the local proxy server (host:port).
    #[serde(default = "default_proxy_bind_addr")]
    pub bind_addr: String,
    /// Base URL exposed to Claude Code (scheme + host + port).
    #[serde(default = "default_proxy_base_url")]
    pub base_url: String,
}

/// Thinking block compatibility settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingConfig {
    #[serde(default)]
    pub mode: ThinkingMode,
    /// Configuration for summarize mode (used when mode = "summarize")
    #[serde(default)]
    pub summarize: SummarizeConfig,
}

/// Configuration for the summarize thinking mode.
///
/// When switching backends, this mode summarizes the session history
/// using an LLM call before the switch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarizeConfig {
    /// Model to use for summarization.
    /// - Specific model name: "claude-3-haiku-20240307", "gpt-4o-mini"
    /// - "current": use the current backend before switching
    #[serde(default = "default_summarize_model")]
    pub model: String,

    /// Backend to use for summarization (if model != "current").
    /// If not specified, uses the backend that has the specified model.
    #[serde(default)]
    pub backend: Option<String>,

    /// Maximum tokens in the generated summary.
    #[serde(default = "default_summarize_max_tokens")]
    pub max_tokens: u32,

    /// Custom prompt for summarization.
    #[serde(default = "default_summarize_prompt")]
    pub prompt: String,
}

/// Terminal display settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalConfig {
    /// Number of lines to keep in scrollback buffer.
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: usize,
}

/// Debug logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugLoggingConfig {
    #[serde(default)]
    pub level: DebugLogLevel,
    #[serde(default)]
    pub format: DebugLogFormat,
    #[serde(default)]
    pub destination: DebugLogDestination,
    #[serde(default = "default_debug_log_file_path")]
    pub file_path: String,
    #[serde(default = "default_debug_body_preview_bytes")]
    pub body_preview_bytes: usize,
    #[serde(default = "default_debug_header_preview")]
    pub header_preview: bool,
    #[serde(default)]
    pub rotation: DebugLogRotation,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DebugLogLevel {
    Off,
    Basic,
    Verbose,
    Full,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DebugLogFormat {
    Console,
    Json,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DebugLogDestination {
    Stderr,
    File,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugLogRotation {
    #[serde(default)]
    pub mode: DebugLogRotationMode,
    #[serde(default = "default_debug_rotation_max_bytes")]
    pub max_bytes: u64,
    #[serde(default = "default_debug_rotation_max_files")]
    pub max_files: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DebugLogRotationMode {
    None,
    Size,
    Daily,
}

/// Handling mode for thinking blocks.
///
/// # Modes
///
/// - `Strip` (recommended): Remove thinking blocks entirely. Simple and compatible.
/// - `Summarize` (future): Keep native during work, summarize on backend switch.
/// - `Native` (future): Keep native format, requires handoff on switch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingMode {
    /// Remove thinking blocks entirely (recommended)
    Strip,
    /// Keep native during work, summarize on backend switch (future)
    Summarize,
    /// Keep native format with handoff on switch (future)
    Native,
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

fn default_scrollback_lines() -> usize {
    10_000
}

fn default_debug_log_file_path() -> String {
    "~/.config/claude-wrapper/debug.log".to_string()
}

fn default_debug_body_preview_bytes() -> usize {
    1024
}

fn default_debug_header_preview() -> bool {
    true
}

fn default_debug_rotation_max_bytes() -> u64 {
    10 * 1024 * 1024
}

fn default_debug_rotation_max_files() -> usize {
    5
}

fn default_proxy_bind_addr() -> String {
    "127.0.0.1:8080".to_string()
}

fn default_proxy_base_url() -> String {
    "http://127.0.0.1:8080".to_string()
}

fn default_summarize_model() -> String {
    "claude-3-haiku-20240307".to_string()
}

fn default_summarize_max_tokens() -> u32 {
    500
}

fn default_summarize_prompt() -> String {
    "Summarize this coding session for handoff to another AI assistant. \
     Focus on: current task, files modified, decisions made, and next steps. \
     Be concise but include all important context."
        .to_string()
}

/// Backend configuration for an API provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backend {
    /// Unique identifier (e.g., "claude", "glm", "openrouter").
    pub name: String,
    /// Display name in UI (e.g., "Claude", "GLM-4").
    pub display_name: String,
    /// Base URL for API (e.g., "https://api.anthropic.com").
    pub base_url: String,
    /// Authentication type: "api_key", "bearer", "none".
    #[serde(rename = "auth_type")]
    pub auth_type_str: String,
    /// Direct API key for this backend.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Optional pricing per million tokens.
    #[serde(default)]
    pub pricing: Option<BackendPricing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

impl Default for Backend {
    fn default() -> Self {
        Self {
            name: "claude".to_string(),
            display_name: "Claude".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            auth_type_str: "passthrough".to_string(),
            api_key: None,
            pricing: None,
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
            proxy: ProxyConfig::default(),
            thinking: ThinkingConfig::default(),
            terminal: TerminalConfig::default(),
            debug_logging: DebugLoggingConfig::default(),
            backends: vec![Backend::default()],
        }
    }
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_proxy_bind_addr(),
            base_url: default_proxy_base_url(),
        }
    }
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            mode: ThinkingMode::Strip,
            summarize: SummarizeConfig::default(),
        }
    }
}

impl Default for SummarizeConfig {
    fn default() -> Self {
        Self {
            model: default_summarize_model(),
            backend: None,
            max_tokens: default_summarize_max_tokens(),
            prompt: default_summarize_prompt(),
        }
    }
}

impl Default for ThinkingMode {
    fn default() -> Self {
        ThinkingMode::Strip
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: default_scrollback_lines(),
        }
    }
}

impl Default for DebugLoggingConfig {
    fn default() -> Self {
        Self {
            level: DebugLogLevel::Off,
            format: DebugLogFormat::Console,
            destination: DebugLogDestination::Stderr,
            file_path: default_debug_log_file_path(),
            body_preview_bytes: default_debug_body_preview_bytes(),
            header_preview: default_debug_header_preview(),
            rotation: DebugLogRotation::default(),
        }
    }
}

impl Default for DebugLogRotation {
    fn default() -> Self {
        Self {
            mode: DebugLogRotationMode::None,
            max_bytes: default_debug_rotation_max_bytes(),
            max_files: default_debug_rotation_max_files(),
        }
    }
}

impl Default for DebugLogLevel {
    fn default() -> Self {
        DebugLogLevel::Off
    }
}

impl Default for DebugLogFormat {
    fn default() -> Self {
        DebugLogFormat::Console
    }
}

impl Default for DebugLogDestination {
    fn default() -> Self {
        DebugLogDestination::Stderr
    }
}

impl Default for DebugLogRotationMode {
    fn default() -> Self {
        DebugLogRotationMode::None
    }
}

impl DebugLoggingConfig {
    pub fn apply_env_overrides(&mut self) {
        if let Ok(value) = std::env::var("CLAUDE_WRAPPER_DEBUG_LEVEL") {
            if let Some(level) = DebugLogLevel::parse(&value) {
                self.level = level;
            }
        }

        if let Ok(value) = std::env::var("CLAUDE_WRAPPER_DEBUG_FORMAT") {
            if let Some(format) = DebugLogFormat::parse(&value) {
                self.format = format;
            }
        }

        if let Ok(value) = std::env::var("CLAUDE_WRAPPER_DEBUG_DEST") {
            if let Some(dest) = DebugLogDestination::parse(&value) {
                self.destination = dest;
            }
        }

        if let Ok(value) = std::env::var("CLAUDE_WRAPPER_DEBUG_FILE") {
            if !value.trim().is_empty() {
                self.file_path = value;
            }
        }

        if let Ok(value) = std::env::var("CLAUDE_WRAPPER_DEBUG_BODY_PREVIEW_BYTES") {
            if let Ok(parsed) = value.parse::<usize>() {
                self.body_preview_bytes = parsed;
            }
        }
    }
}

impl DebugLogLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "off" => Some(DebugLogLevel::Off),
            "basic" => Some(DebugLogLevel::Basic),
            "verbose" => Some(DebugLogLevel::Verbose),
            "full" => Some(DebugLogLevel::Full),
            _ => None,
        }
    }
}

impl DebugLogFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "console" => Some(DebugLogFormat::Console),
            "json" | "jsonl" => Some(DebugLogFormat::Json),
            _ => None,
        }
    }
}

impl DebugLogDestination {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "stderr" => Some(DebugLogDestination::Stderr),
            "file" => Some(DebugLogDestination::File),
            "both" => Some(DebugLogDestination::Both),
            _ => None,
        }
    }
}
