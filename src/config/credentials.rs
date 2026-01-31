//! Credential resolution from config and environment variables.
//!
//! This module provides secure handling of API keys and credentials
//! resolved from config or environment variables at runtime.

use std::env;

use super::types::Backend;

/// Authentication type for API requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
    /// Anthropic-style `x-api-key` header.
    ApiKey,
    /// Standard `Authorization: Bearer` header.
    Bearer,
    /// No authentication required.
    None,
    /// Pass through original auth headers from client.
    Passthrough,
}

impl AuthType {
    /// Parse auth type from string.
    /// Defaults to `ApiKey` for unknown values.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "bearer" => AuthType::Bearer,
            "none" => AuthType::None,
            "passthrough" => AuthType::Passthrough,
            _ => AuthType::ApiKey,
        }
    }
}

/// Wrapper for sensitive strings that prevents accidental logging.
///
/// The inner value is never exposed via Debug or Display traits.
/// Use `expose()` to access the actual value when needed for API calls.
#[derive(Clone)]
pub struct SecureString(String);

impl SecureString {
    /// Create a new secure string.
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Expose the inner value.
    ///
    /// Use sparingly and only when actually sending to APIs.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecureString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecureString(••••••••)")
    }
}

impl std::fmt::Display for SecureString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "••••••••")
    }
}

/// Status of credential resolution for a backend.
#[derive(Debug, Clone)]
pub enum CredentialStatus {
    /// API key resolved successfully.
    Configured(SecureString),
    /// Environment variable is missing or empty.
    Unconfigured {
        /// Name of the missing environment variable.
        env_var: String,
    },
    /// No authentication required for this backend.
    NoAuth,
}

impl Backend {
    /// Parse the auth_type field to AuthType enum.
    pub fn auth_type(&self) -> AuthType {
        AuthType::from_str(&self.auth_type_str)
    }

    /// Resolve the API key from environment variable.
    ///
    /// This is called on-demand and NOT cached, enabling hot-reload
    /// of credentials when environment variables change.
    pub fn resolve_credential(&self) -> CredentialStatus {
        match self.auth_type() {
            AuthType::None | AuthType::Passthrough => CredentialStatus::NoAuth,
            AuthType::ApiKey | AuthType::Bearer => {
                if let Some(value) = self.api_key.as_ref().filter(|value| !value.is_empty()) {
                    return CredentialStatus::Configured(SecureString::new(value.clone()));
                }

                match env::var(&self.auth_env_var) {
                    Ok(value) if !value.is_empty() => {
                        CredentialStatus::Configured(SecureString::new(value))
                    }
                    _ => CredentialStatus::Unconfigured {
                        env_var: self.auth_env_var.clone(),
                    },
                }
            }
        }
    }

    /// Check if this backend is configured (has valid credentials or doesn't need them).
    pub fn is_configured(&self) -> bool {
        matches!(
            self.resolve_credential(),
            CredentialStatus::Configured(_) | CredentialStatus::NoAuth
        )
    }

    /// Describe how to provide credentials for this backend.
    pub fn missing_credential_hint(&self) -> String {
        if self.auth_env_var.is_empty() {
            "api_key".to_string()
        } else {
            format!("api_key or {} environment variable", self.auth_env_var)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_type_parsing() {
        assert_eq!(AuthType::from_str("api_key"), AuthType::ApiKey);
        assert_eq!(AuthType::from_str("bearer"), AuthType::Bearer);
        assert_eq!(AuthType::from_str("Bearer"), AuthType::Bearer);
        assert_eq!(AuthType::from_str("none"), AuthType::None);
        assert_eq!(AuthType::from_str("NONE"), AuthType::None);
        assert_eq!(AuthType::from_str("unknown"), AuthType::ApiKey);
        assert_eq!(AuthType::from_str(""), AuthType::ApiKey);
    }

    #[test]
    fn test_secure_string_does_not_leak() {
        let secret = SecureString::new("my-secret-key".to_string());

        // Debug should mask
        let debug_output = format!("{:?}", secret);
        assert!(!debug_output.contains("my-secret-key"));
        assert!(debug_output.contains("••••••••"));

        // Display should mask
        let display_output = format!("{}", secret);
        assert!(!display_output.contains("my-secret-key"));
        assert!(display_output.contains("••••••••"));

        // expose() should reveal
        assert_eq!(secret.expose(), "my-secret-key");
    }

    #[test]
    fn test_credential_resolution_no_auth() {
        let backend = Backend {
            name: "test".to_string(),
            display_name: "Test".to_string(),
            base_url: "https://example.com".to_string(),
            auth_type_str: "none".to_string(),
            api_key: None,
            auth_env_var: "".to_string(),
        };

        assert!(matches!(
            backend.resolve_credential(),
            CredentialStatus::NoAuth
        ));
        assert!(backend.is_configured());
    }

    #[test]
    fn test_credential_resolution_prefers_api_key() {
        let env_var = "TEST_API_KEY_ENV_FALLBACK";
        std::env::set_var(env_var, "env-key-value");

        let backend = Backend {
            name: "test".to_string(),
            display_name: "Test".to_string(),
            base_url: "https://example.com".to_string(),
            auth_type_str: "api_key".to_string(),
            api_key: Some("direct-key-value".to_string()),
            auth_env_var: env_var.to_string(),
            models: vec![],
        };

        match backend.resolve_credential() {
            CredentialStatus::Configured(value) => {
                assert_eq!(value.expose(), "direct-key-value");
            }
            _ => panic!("Expected configured credential"),
        }

        std::env::remove_var(env_var);
    }

    #[test]
    fn test_credential_resolution_falls_back_to_env() {
        let env_var = "TEST_API_KEY_FALLBACK";
        std::env::set_var(env_var, "env-key-value");

        let backend = Backend {
            name: "test".to_string(),
            display_name: "Test".to_string(),
            base_url: "https://example.com".to_string(),
            auth_type_str: "api_key".to_string(),
            api_key: Some("".to_string()),
            auth_env_var: env_var.to_string(),
            models: vec![],
        };

        match backend.resolve_credential() {
            CredentialStatus::Configured(value) => {
                assert_eq!(value.expose(), "env-key-value");
            }
            _ => panic!("Expected configured credential"),
        }

        std::env::remove_var(env_var);
    }
}
