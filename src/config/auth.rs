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
        (AuthType::Passthrough, _) => None, // Auth comes from client
        (_, CredentialStatus::Unconfigured { .. }) => None,
        (_, CredentialStatus::NoAuth) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn make_backend(auth_type: &str, env_var: &str) -> Backend {
        Backend {
            name: "test".to_string(),
            display_name: "Test".to_string(),
            base_url: "https://example.com".to_string(),
            auth_type_str: auth_type.to_string(),
            auth_env_var: env_var.to_string(),
            api_key: None,
            models: vec![],
        }
    }

    #[test]
    fn test_no_auth_backend() {
        let backend = make_backend("none", "");
        assert!(build_auth_header(&backend).is_none());
    }

    #[test]
    fn test_api_key_header() {
        let env_var = "TEST_API_KEY_HEADER";
        env::set_var(env_var, "test-key-123");

        let backend = make_backend("api_key", env_var);
        let header = build_auth_header(&backend);

        assert!(header.is_some());
        let (name, value) = header.unwrap();
        assert_eq!(name, "x-api-key");
        assert_eq!(value, "test-key-123");

        env::remove_var(env_var);
    }

    #[test]
    fn test_bearer_header() {
        let env_var = "TEST_BEARER_KEY";
        env::set_var(env_var, "bearer-token-456");

        let backend = make_backend("bearer", env_var);
        let header = build_auth_header(&backend);

        assert!(header.is_some());
        let (name, value) = header.unwrap();
        assert_eq!(name, "Authorization");
        assert_eq!(value, "Bearer bearer-token-456");

        env::remove_var(env_var);
    }

    #[test]
    fn test_missing_env_var() {
        let backend = make_backend("api_key", "NONEXISTENT_VAR_XYZ123");
        assert!(build_auth_header(&backend).is_none());
    }

    #[test]
    fn test_empty_env_var() {
        let env_var = "TEST_EMPTY_VAR";
        env::set_var(env_var, "");

        let backend = make_backend("api_key", env_var);
        assert!(build_auth_header(&backend).is_none());

        env::remove_var(env_var);
    }
}
