//! Authentication header building for API requests.
//!
//! Builds the appropriate authentication headers based on
//! backend configuration and resolved credentials.

use super::credentials::{AuthType, CredentialStatus};
use super::types::Backend;

/// Header name and value for authentication.
pub type AuthHeader = (String, String);

/// Build the authentication header for a backend.
///
/// Returns `Some((header_name, header_value))` if auth is configured,
/// or `None` if no auth is needed or credentials are missing.
pub fn build_auth_header(backend: &Backend) -> Option<AuthHeader> {
    let cred = backend.resolve_credential();
    let auth_type = backend.auth_type();

    match (auth_type, cred) {
        (AuthType::ApiKey, CredentialStatus::Configured(key)) => {
            Some(("x-api-key".to_string(), key.expose().to_string()))
        }
        (AuthType::Bearer, CredentialStatus::Configured(key)) => Some((
            "Authorization".to_string(),
            format!("Bearer {}", key.expose()),
        )),
        (AuthType::None, _) => None,
        (_, CredentialStatus::Unconfigured { .. }) => None,
        (_, CredentialStatus::NoAuth) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_backend(auth_type: &str, api_key: Option<&str>) -> Backend {
        Backend {
            name: "test".to_string(),
            display_name: "Test".to_string(),
            base_url: "https://example.com".to_string(),
            auth_type_str: auth_type.to_string(),
            api_key: api_key.map(|value| value.to_string()),
            models: vec![],
        }
    }

    #[test]
    fn test_no_auth_backend() {
        let backend = make_backend("none", None);
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
}
