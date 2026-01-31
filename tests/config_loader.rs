use claudewrapper::config::{
    build_auth_header, AuthType, Backend, Config, ConfigError, CredentialStatus, Defaults,
    ProxyConfig,
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
    assert_eq!(backend.api_key, None);
    assert_eq!(backend.auth_env_var, "ANTHROPIC_API_KEY");
}

/// Test that Config::config_path() returns a path ending with the expected filename.
#[test]
fn test_config_path_ends_with_expected() {
    let path = Config::config_path();
    assert!(path.ends_with("claude-wrapper/config.toml"));
}

/// Test validation passes for default config when env var is set.
#[test]
fn test_validation_passes_for_default() {
    // Set env var so the default backend is configured
    std::env::set_var("ANTHROPIC_API_KEY", "test-key");
    let config = Config::default();
    let result = config.validate();
    std::env::remove_var("ANTHROPIC_API_KEY");
    assert!(result.is_ok());
}

/// Test validation fails when no backends are configured.
#[test]
fn test_validation_fails_empty_backends() {
    let config = Config {
        defaults: Defaults::default(),
        proxy: ProxyConfig::default(),
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
auth_env_var = "ANTHROPIC_API_KEY"
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
// Environment Variable Resolution Tests
// ============================================================================

/// Test that backend is_configured returns true when env var is set.
#[test]
fn test_backend_is_configured_with_env_var() {
    let env_var = "TEST_CONFIGURED_API_KEY";
    std::env::set_var(env_var, "test-key-value");

    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: "api_key".to_string(),
        api_key: None,
        auth_env_var: env_var.to_string(),
        api_key: None,
    };

    assert!(backend.is_configured());

    std::env::remove_var(env_var);
}

/// Test that backend is_configured returns false when env var is missing.
#[test]
fn test_backend_not_configured_without_env_var() {
    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: "api_key".to_string(),
        api_key: None,
        auth_env_var: "NONEXISTENT_ENV_VAR_XYZ".to_string(),
        api_key: None,
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
        auth_env_var: "".to_string(),
        api_key: None,
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
    let env_var = "TEST_AUTH_HEADER_API_KEY";
    std::env::set_var(env_var, "my-secret-key");

    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: "api_key".to_string(),
        api_key: None,
        auth_env_var: env_var.to_string(),
        api_key: None,
    };

    let header = build_auth_header(&backend);
    assert!(header.is_some());

    let (name, value) = header.unwrap();
    assert_eq!(name, "x-api-key");
    assert_eq!(value, "my-secret-key");

    std::env::remove_var(env_var);
}

/// Test build_auth_header creates correct Bearer header.
#[test]
fn test_build_auth_header_bearer() {
    let env_var = "TEST_AUTH_HEADER_BEARER";
    std::env::set_var(env_var, "my-bearer-token");

    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: "bearer".to_string(),
        api_key: None,
        auth_env_var: env_var.to_string(),
        api_key: None,
    };

    let header = build_auth_header(&backend);
    assert!(header.is_some());

    let (name, value) = header.unwrap();
    assert_eq!(name, "Authorization");
    assert_eq!(value, "Bearer my-bearer-token");

    std::env::remove_var(env_var);
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
        backends: vec![Backend {
            name: "unconfigured".to_string(),
            display_name: "Unconfigured".to_string(),
            base_url: "https://example.com".to_string(),
            auth_type_str: "api_key".to_string(),
            api_key: None,
            auth_env_var: "NONEXISTENT_VAR_ABC123".to_string(),
            api_key: None,
        }],
    };

    let result = config.validate();
    assert!(result.is_err());

    match result.unwrap_err() {
        ConfigError::ValidationError { message } => {
            assert!(message.contains("not configured"));
            assert!(message.contains("NONEXISTENT_VAR_ABC123"));
        }
        _ => panic!("Expected ValidationError"),
    }
}

/// Test configured_backends only returns backends with valid credentials.
#[test]
fn test_configured_backends_filters_correctly() {
    let env_var = "TEST_CONFIGURED_FILTER";
    std::env::set_var(env_var, "test-key");

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
        backends: vec![
            Backend {
                name: "configured".to_string(),
                display_name: "Configured".to_string(),
                base_url: "https://example.com".to_string(),
                auth_type_str: "api_key".to_string(),
                api_key: None,
                auth_env_var: env_var.to_string(),
                api_key: None,
            },
            Backend {
                name: "unconfigured".to_string(),
                display_name: "Unconfigured".to_string(),
                base_url: "https://example.com".to_string(),
                auth_type_str: "api_key".to_string(),
                api_key: None,
                auth_env_var: "NONEXISTENT_VAR_XYZ789".to_string(),
                api_key: None,
            },
            Backend {
                name: "no-auth".to_string(),
                display_name: "No Auth".to_string(),
                base_url: "https://example.com".to_string(),
                auth_type_str: "none".to_string(),
                api_key: None,
                auth_env_var: "".to_string(),
                api_key: None,
            },
        ],
    };

    let configured = config.configured_backends();

    // Should have 2 configured backends (one with key, one with no-auth)
    assert_eq!(configured.len(), 2);
    assert!(configured.iter().any(|b| b.name == "configured"));
    assert!(configured.iter().any(|b| b.name == "no-auth"));
    assert!(!configured.iter().any(|b| b.name == "unconfigured"));

    std::env::remove_var(env_var);
}
