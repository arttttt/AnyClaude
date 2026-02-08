mod common;

use anyclaude::config::{AuthType, Backend, CredentialStatus, SecureString};

#[test]
fn test_auth_type_parsing() {
    assert_eq!(AuthType::parse("api_key"), AuthType::ApiKey);
    assert_eq!(AuthType::parse("API_KEY"), AuthType::ApiKey);
    assert_eq!(AuthType::parse("bearer"), AuthType::Bearer);
    assert_eq!(AuthType::parse("Bearer"), AuthType::Bearer);
    assert_eq!(AuthType::parse("passthrough"), AuthType::Passthrough);
    assert_eq!(AuthType::parse("PASSTHROUGH"), AuthType::Passthrough);
    // Unknown values default to Passthrough (safe for OAuth)
    assert_eq!(AuthType::parse("unknown"), AuthType::Passthrough);
    assert_eq!(AuthType::parse(""), AuthType::Passthrough);
}

#[test]
fn test_secure_string_does_not_leak() {
    let secret = SecureString::new("my-secret-key".to_string());

    // Debug should mask
    let debug_output = format!("{:?}", secret);
    assert!(!debug_output.contains("my-secret-key"));
    assert!(debug_output.contains("\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"));

    // Display should mask
    let display_output = format!("{}", secret);
    assert!(!display_output.contains("my-secret-key"));
    assert!(display_output.contains("\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"));

    // expose() should reveal
    assert_eq!(secret.expose(), "my-secret-key");
}

#[test]
fn test_uses_own_credentials() {
    // Passthrough forwards client auth headers unchanged
    assert!(!AuthType::Passthrough.uses_own_credentials());

    // ApiKey and Bearer use backend's configured credentials
    assert!(AuthType::ApiKey.uses_own_credentials());
    assert!(AuthType::Bearer.uses_own_credentials());
}

#[test]
fn test_credential_resolution_passthrough() {
    let backend = Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
    };

    assert!(matches!(
        backend.resolve_credential(),
        CredentialStatus::NoAuth
    ));
    assert!(backend.is_configured());
}
