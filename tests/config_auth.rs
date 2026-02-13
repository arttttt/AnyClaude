mod common;

use anyclaude::config::{build_auth_header, Backend};

fn make_backend(auth_type: &str, api_key: Option<&str>) -> Backend {
    Backend {
        name: "test".to_string(),
        display_name: "Test".to_string(),
        base_url: "https://example.com".to_string(),
        auth_type_str: auth_type.to_string(),
        api_key: api_key.map(|value| value.to_string()),
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    }
}

#[test]
fn test_passthrough_backend() {
    let backend = make_backend("passthrough", None);
    assert!(build_auth_header(&backend).is_none());
}

#[test]
fn test_api_key_header() {
    let backend = make_backend("api_key", Some("test-key-123"));
    let header = build_auth_header(&backend);

    assert!(header.is_some());
    let (name, value) = header.unwrap();
    assert_eq!(name, "x-api-key");
    assert_eq!(value, "test-key-123");
}

#[test]
fn test_bearer_header() {
    let backend = make_backend("bearer", Some("bearer-token-456"));
    let header = build_auth_header(&backend);

    assert!(header.is_some());
    let (name, value) = header.unwrap();
    assert_eq!(name, "Authorization");
    assert_eq!(value, "Bearer bearer-token-456");
}

#[test]
fn test_missing_api_key() {
    let backend = make_backend("api_key", None);
    assert!(build_auth_header(&backend).is_none());
}

#[test]
fn test_empty_api_key() {
    let backend = make_backend("api_key", Some(""));
    assert!(build_auth_header(&backend).is_none());
}
