use claudewrapper::config::{
    build_auth_header, AuthType, Backend, Config, ConfigError, CredentialStatus, Defaults,
    ProxyConfig, ThinkingConfig,
};

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
    assert_eq!(backend.auth_type(), AuthType::ApiKey);
    assert!(backend.api_key.is_none());
    assert_eq!(backend.models, vec!["claude-sonnet-4-20250514"]);
}

/// Test that Config::config_path() returns a path ending with the expected filename.
#[test]
fn test_config_path_ends_with_expected() {
    let path = Config::config_path();
    assert!(path.ends_with("claude-wrapper/config.toml"));
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
        thinking: ThinkingConfig::default(),
        backends: vec![],
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
        thinking: ThinkingConfig::default(),
        backends: vec![Backend::default()],
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
models = ["claude-sonnet-4-20250514", "claude-3-opus-20240229"]
"#;

    let config: Config = toml::from_str(toml_content).expect("Should parse valid TOML");

    assert_eq!(config.defaults.active, "claude");
    assert_eq!(config.defaults.timeout_seconds, 60);
    assert_eq!(config.backends.len(), 1);
    assert_eq!(config.backends[0].models.len(), 2);
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
        models: vec![],
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
        models: vec![],
    };

    assert!(!backend.is_configured());
}

/// Test that backend with auth_type "none" is always configured.
#[test]
fn test_backend_no_auth_always_configured() {
    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: "none".to_string(),
        api_key: None,
        models: vec![],
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
        models: vec![],
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
        models: vec![],
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
        thinking: ThinkingConfig::default(),
        backends: vec![Backend {
            name: "unconfigured".to_string(),
            display_name: "Unconfigured".to_string(),
            base_url: "https://example.com".to_string(),
            auth_type_str: "api_key".to_string(),
            api_key: None,
            models: vec![],
        }],
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
        thinking: ThinkingConfig::default(),
        backends: vec![
            Backend {
                name: "configured".to_string(),
                display_name: "Configured".to_string(),
                base_url: "https://example.com".to_string(),
                auth_type_str: "api_key".to_string(),
                api_key: Some("test-key".to_string()),
                models: vec![],
            },
            Backend {
                name: "unconfigured".to_string(),
                display_name: "Unconfigured".to_string(),
                base_url: "https://example.com".to_string(),
                auth_type_str: "api_key".to_string(),
                api_key: None,
                models: vec![],
            },
            Backend {
                name: "no-auth".to_string(),
                display_name: "No Auth".to_string(),
                base_url: "https://example.com".to_string(),
                auth_type_str: "none".to_string(),
                api_key: None,
                models: vec![],
            },
        ],
    };

    let configured = config.configured_backends();

    // Should have 2 configured backends (one with key, one with no-auth)
    assert_eq!(configured.len(), 2);
    assert!(configured.iter().any(|b| b.name == "configured"));
    assert!(configured.iter().any(|b| b.name == "no-auth"));
    assert!(!configured.iter().any(|b| b.name == "unconfigured"));
}
