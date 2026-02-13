use anyclaude::config::{
    build_auth_header, AgentTeamsConfig, AuthType, Backend, Config, ConfigError,
    CredentialStatus, DebugLoggingConfig, Defaults, ProxyConfig, TerminalConfig,
};
use std::collections::HashMap;

/// Test that Config::default() produces the expected values per spec.
#[test]
fn test_config_default_values() {
    let config = Config::default();

    // Defaults
    assert_eq!(config.defaults.active, "claude");
    assert_eq!(config.defaults.timeout_seconds, 30);
    assert_eq!(config.defaults.pool_idle_timeout_seconds, 90);
    assert_eq!(config.defaults.pool_max_idle_per_host, 8);
    assert_eq!(config.defaults.max_retries, 3);
    assert_eq!(config.defaults.retry_backoff_base_ms, 100);

    // Should have exactly one backend
    assert_eq!(config.backends.len(), 1);

    let backend = &config.backends[0];
    assert_eq!(backend.name, "claude");
    assert_eq!(backend.display_name, "Claude");
    assert_eq!(backend.base_url, "https://api.anthropic.com");
    assert_eq!(backend.auth_type(), AuthType::Passthrough);
    assert!(backend.api_key.is_none());
    // models field was removed - proxy doesn't manage available models
}

/// Test that Config::config_path() returns a path ending with the expected filename.
#[test]
fn test_config_path_ends_with_expected() {
    let path = Config::config_path();
    assert!(path.ends_with("anyclaude/config.toml"));
}

/// Test validation passes for default config when api_key is set.
#[test]
fn test_validation_passes_for_default() {
    let mut config = Config::default();
    config.backends[0].api_key = Some("test-key".to_string());
    let result = config.validate();
    assert!(result.is_ok());
}

/// Test validation fails when no backends are configured.
#[test]
fn test_validation_fails_empty_backends() {
    let config = Config {
        defaults: Defaults::default(),
        proxy: ProxyConfig::default(),
        terminal: TerminalConfig::default(),
        debug_logging: DebugLoggingConfig::default(),
        claude_settings: HashMap::new(),
        backends: vec![],
        agent_teams: None,
    };

    let result = config.validate();
    assert!(result.is_err());

    match result.unwrap_err() {
        ConfigError::ValidationError { message } => {
            assert!(message.contains("At least one backend"));
        }
        _ => panic!("Expected ValidationError"),
    }
}

/// Test validation fails when active backend doesn't exist.
#[test]
fn test_validation_fails_missing_active_backend() {
    let config = Config {
        defaults: Defaults {
            active: "nonexistent".to_string(),
            timeout_seconds: 30,
            connect_timeout_seconds: 5,
            idle_timeout_seconds: 60,
            pool_idle_timeout_seconds: 90,
            pool_max_idle_per_host: 8,
            max_retries: 3,
            retry_backoff_base_ms: 100,
        },
        proxy: ProxyConfig::default(),
        terminal: TerminalConfig::default(),
        debug_logging: DebugLoggingConfig::default(),
        claude_settings: HashMap::new(),
        backends: vec![Backend::default()],
        agent_teams: None,
    };

    let result = config.validate();
    assert!(result.is_err());

    match result.unwrap_err() {
        ConfigError::ValidationError { message } => {
            assert!(message.contains("nonexistent"));
            assert!(message.contains("not found"));
        }
        _ => panic!("Expected ValidationError"),
    }
}

/// Test that valid TOML parses correctly.
#[test]
fn test_parse_valid_toml() {
    let toml_content = r#"
[defaults]
active = "claude"
timeout_seconds = 60

[[backends]]
name = "claude"
display_name = "Claude"
base_url = "https://api.anthropic.com"
auth_type = "api_key"
api_key = "test-key-123"
"#;

    let config: Config = toml::from_str(toml_content).expect("Should parse valid TOML");

    assert_eq!(config.defaults.active, "claude");
    assert_eq!(config.defaults.timeout_seconds, 60);
    assert_eq!(config.backends.len(), 1);
}

/// Test that invalid TOML produces a parse error.
#[test]
fn test_parse_invalid_toml() {
    let invalid_toml = "this is not valid toml [[[";

    let result: Result<Config, _> = toml::from_str(invalid_toml);
    assert!(result.is_err());
}

/// Test round-trip serialization/deserialization.
#[test]
fn test_config_roundtrip() {
    let original = Config::default();
    let serialized = toml::to_string(&original).expect("Should serialize");
    let deserialized: Config = toml::from_str(&serialized).expect("Should deserialize");

    assert_eq!(original.defaults.active, deserialized.defaults.active);
    assert_eq!(
        original.defaults.timeout_seconds,
        deserialized.defaults.timeout_seconds
    );
    assert_eq!(original.backends.len(), deserialized.backends.len());
    assert_eq!(original.backends[0].name, deserialized.backends[0].name);
}

// ============================================================================
// API Key Resolution Tests
// ============================================================================

/// Test that backend is_configured returns true when api_key is set.
#[test]
fn test_backend_is_configured_with_api_key() {
    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: "api_key".to_string(),
        api_key: Some("test-key-value".to_string()),
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    assert!(backend.is_configured());
}

/// Test that backend is_configured returns false when api_key is missing.
#[test]
fn test_backend_not_configured_without_api_key() {
    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: "api_key".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    assert!(!backend.is_configured());
}

/// Test that backend with auth_type "passthrough" is always configured.
#[test]
fn test_backend_passthrough_always_configured() {
    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    assert!(backend.is_configured());
    assert!(matches!(
        backend.resolve_credential(),
        CredentialStatus::NoAuth
    ));
}

/// Test build_auth_header creates correct x-api-key header.
#[test]
fn test_build_auth_header_api_key() {
    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: "api_key".to_string(),
        api_key: Some("my-secret-key".to_string()),
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    let header = build_auth_header(&backend);
    assert!(header.is_some());

    let (name, value) = header.unwrap();
    assert_eq!(name, "x-api-key");
    assert_eq!(value, "my-secret-key");
}

/// Test build_auth_header creates correct Bearer header.
#[test]
fn test_build_auth_header_bearer() {
    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: "bearer".to_string(),
        api_key: Some("my-bearer-token".to_string()),
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    };

    let header = build_auth_header(&backend);
    assert!(header.is_some());

    let (name, value) = header.unwrap();
    assert_eq!(name, "Authorization");
    assert_eq!(value, "Bearer my-bearer-token");
}

/// Test validation fails when active backend is unconfigured.
#[test]
fn test_validation_fails_unconfigured_active_backend() {
    let config = Config {
        defaults: Defaults {
            active: "unconfigured".to_string(),
            timeout_seconds: 30,
            connect_timeout_seconds: 5,
            idle_timeout_seconds: 60,
            pool_idle_timeout_seconds: 90,
            pool_max_idle_per_host: 8,
            max_retries: 3,
            retry_backoff_base_ms: 100,
        },
        proxy: ProxyConfig::default(),
        terminal: TerminalConfig::default(),
        debug_logging: DebugLoggingConfig::default(),
        claude_settings: HashMap::new(),
        backends: vec![Backend {
            name: "unconfigured".to_string(),
            display_name: "Unconfigured".to_string(),
            base_url: "https://example.com".to_string(),
            auth_type_str: "api_key".to_string(),
            api_key: None,
            pricing: None,
            thinking_compat: None,
            thinking_budget_tokens: None,
            model_opus: None,
            model_sonnet: None,
            model_haiku: None,
        }],
        agent_teams: None,
    };

    let result = config.validate();
    assert!(result.is_err());

    match result.unwrap_err() {
        ConfigError::ValidationError { message } => {
            assert!(message.contains("not configured"));
            assert!(message.contains("api_key"));
        }
        _ => panic!("Expected ValidationError"),
    }
}

/// Test validation fails when agent_teams.teammate_backend references a nonexistent backend.
#[test]
fn test_validation_fails_invalid_teammate_backend() {
    let config = Config {
        defaults: Defaults::default(),
        proxy: ProxyConfig::default(),
        terminal: TerminalConfig::default(),
        debug_logging: DebugLoggingConfig::default(),
        claude_settings: HashMap::new(),
        backends: vec![Backend::default()],
        agent_teams: Some(AgentTeamsConfig {
            teammate_backend: "nonexistent".to_string(),
        }),
    };

    let result = config.validate();
    assert!(result.is_err());

    match result.unwrap_err() {
        ConfigError::ValidationError { message } => {
            assert!(message.contains("nonexistent"), "got: {message}");
            assert!(message.contains("not found"), "got: {message}");
        }
        other => panic!("Expected ValidationError, got: {other:?}"),
    }
}

/// Test validation fails when TOML config has agent_teams with nonexistent backend.
/// This tests the real user flow: write TOML → parse → validate.
#[test]
fn test_validation_fails_invalid_teammate_backend_from_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[defaults]
active = "claude"
timeout_seconds = 30

[[backends]]
name = "claude"
display_name = "Claude"
base_url = "https://api.anthropic.com"
auth_type = "passthrough"

[agent_teams]
teammate_backend = "nonexistent"
"#,
    )
    .unwrap();

    let result = Config::load_from(&path);
    assert!(result.is_err(), "should reject nonexistent teammate_backend");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("nonexistent"), "got: {err}");
}

/// Test validation passes when agent_teams.teammate_backend references an existing backend.
#[test]
fn test_validation_passes_valid_teammate_backend() {
    let config = Config {
        defaults: Defaults::default(),
        proxy: ProxyConfig::default(),
        terminal: TerminalConfig::default(),
        debug_logging: DebugLoggingConfig::default(),
        claude_settings: HashMap::new(),
        backends: vec![Backend::default()],
        agent_teams: Some(AgentTeamsConfig {
            teammate_backend: "claude".to_string(),
        }),
    };

    assert!(config.validate().is_ok());
}

/// Test configured_backends only returns backends with valid credentials.
#[test]
fn test_configured_backends_filters_correctly() {
    let config = Config {
        defaults: Defaults {
            active: "configured".to_string(),
            timeout_seconds: 30,
            connect_timeout_seconds: 5,
            idle_timeout_seconds: 60,
            pool_idle_timeout_seconds: 90,
            pool_max_idle_per_host: 8,
            max_retries: 3,
            retry_backoff_base_ms: 100,
        },
        proxy: ProxyConfig::default(),
        terminal: TerminalConfig::default(),
        debug_logging: DebugLoggingConfig::default(),
        claude_settings: HashMap::new(),
        backends: vec![
            Backend {
                name: "configured".to_string(),
                display_name: "Configured".to_string(),
                base_url: "https://example.com".to_string(),
                auth_type_str: "api_key".to_string(),
                api_key: Some("test-key".to_string()),
                pricing: None,
                thinking_compat: None,
                thinking_budget_tokens: None,
                model_opus: None,
                model_sonnet: None,
                model_haiku: None,
            },
            Backend {
                name: "unconfigured".to_string(),
                display_name: "Unconfigured".to_string(),
                base_url: "https://example.com".to_string(),
                auth_type_str: "api_key".to_string(),
                api_key: None,
                pricing: None,
                thinking_compat: None,
                thinking_budget_tokens: None,
                model_opus: None,
                model_sonnet: None,
                model_haiku: None,
            },
            Backend {
                name: "passthrough".to_string(),
                display_name: "Passthrough".to_string(),
                base_url: "https://example.com".to_string(),
                auth_type_str: "passthrough".to_string(),
                api_key: None,
                pricing: None,
                thinking_compat: None,
                thinking_budget_tokens: None,
                model_opus: None,
                model_sonnet: None,
                model_haiku: None,
            },
        ],
        agent_teams: None,
    };

    let configured = config.configured_backends();

    // Should have 2 configured backends (one with key, one with passthrough)
    assert_eq!(configured.len(), 2);
    assert!(configured.iter().any(|b| b.name == "configured"));
    assert!(configured.iter().any(|b| b.name == "passthrough"));
    assert!(!configured.iter().any(|b| b.name == "unconfigured"));
}

// ============================================================================
// Model Map Tests
// ============================================================================

fn glm_backend() -> Backend {
    Backend {
        name: "glm".to_string(),
        display_name: "GLM".to_string(),
        base_url: "https://open.bigmodel.cn/api/paas/v4".to_string(),
        auth_type_str: "bearer".to_string(),
        api_key: Some("key".to_string()),
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: Some("glm-4.7".to_string()),
        model_sonnet: Some("glm-4.7".to_string()),
        model_haiku: Some("glm-4.5-air".to_string()),
    }
}

#[test]
fn resolve_model_opus_family() {
    let b = glm_backend();
    assert_eq!(b.resolve_model("claude-opus-4-6"), Some("glm-4.7"));
}

#[test]
fn resolve_model_sonnet_family() {
    let b = glm_backend();
    assert_eq!(b.resolve_model("claude-sonnet-4-5-20250929"), Some("glm-4.7"));
}

#[test]
fn resolve_model_haiku_family() {
    let b = glm_backend();
    assert_eq!(b.resolve_model("claude-haiku-4-5-20251001"), Some("glm-4.5-air"));
}

#[test]
fn resolve_model_bedrock_id() {
    let b = glm_backend();
    assert_eq!(b.resolve_model("us.anthropic.claude-opus-4-5-v1:0"), Some("glm-4.7"));
}

#[test]
fn resolve_model_unknown_passthrough() {
    let b = glm_backend();
    assert_eq!(b.resolve_model("gpt-4o"), None);
}

#[test]
fn resolve_model_no_map() {
    let b = Backend::default();
    assert_eq!(b.resolve_model("claude-opus-4-6"), None);
}

#[test]
fn resolve_model_partial_map() {
    let b = Backend {
        model_opus: Some("mapped-opus".to_string()),
        ..Backend::default()
    };
    assert_eq!(b.resolve_model("claude-opus-4-6"), Some("mapped-opus"));
    assert_eq!(b.resolve_model("claude-sonnet-4-5-20250929"), None);
    assert_eq!(b.resolve_model("claude-haiku-4-5-20251001"), None);
}

#[test]
fn resolve_model_toml_parsing() {
    let toml_content = r#"
[defaults]
active = "glm"
timeout_seconds = 30

[[backends]]
name = "glm"
display_name = "GLM"
base_url = "https://open.bigmodel.cn/api/paas/v4"
auth_type = "bearer"
api_key = "test-key"
model_opus = "glm-4.7"
model_haiku = "glm-4.5-air"
"#;
    let config: Config = toml::from_str(toml_content).expect("Should parse");
    let b = &config.backends[0];
    assert_eq!(b.resolve_model("claude-opus-4-6"), Some("glm-4.7"));
    assert_eq!(b.resolve_model("claude-sonnet-4-5-20250929"), None);
    assert_eq!(b.resolve_model("claude-haiku-4-5-20251001"), Some("glm-4.5-air"));
}

