//! Credential resolution from configuration.
//!
//! This module provides secure handling of API keys and credentials
//! resolved from the config at runtime.

use super::types::Backend;

/// Authentication type for API requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
    /// Anthropic-style `x-api-key` header.
    ApiKey,
    /// Standard `Authorization: Bearer` header.
    Bearer,
    /// Passthrough: forward original client headers unchanged (for OAuth).
    Passthrough,
}

impl AuthType {
    /// Parse auth type from string.
    /// Defaults to `Passthrough` for unknown values (safe default for Anthropic OAuth).
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "api_key" => AuthType::ApiKey,
            "bearer" => AuthType::Bearer,
            _ => AuthType::Passthrough,
        }
    }

    /// Returns true if this auth type uses its own credentials.
    ///
    /// When true, incoming auth headers should be stripped and replaced
    /// with the backend's configured credentials.
    pub fn uses_own_credentials(&self) -> bool {
        matches!(self, AuthType::ApiKey | AuthType::Bearer)
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
    /// API key is missing or empty.
    Unconfigured {
        /// Reason for missing configuration.
        reason: String,
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
            AuthType::Passthrough => CredentialStatus::NoAuth,
            AuthType::ApiKey | AuthType::Bearer => {
                if let Some(ref key) = self.api_key {
                    if !key.is_empty() {
                        return CredentialStatus::Configured(SecureString::new(key.clone()));
                    }
                }
                CredentialStatus::Unconfigured {
                    reason: "api_key is not set".to_string(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_type_parsing() {
        assert_eq!(AuthType::from_str("api_key"), AuthType::ApiKey);
        assert_eq!(AuthType::from_str("API_KEY"), AuthType::ApiKey);
        assert_eq!(AuthType::from_str("bearer"), AuthType::Bearer);
        assert_eq!(AuthType::from_str("Bearer"), AuthType::Bearer);
        assert_eq!(AuthType::from_str("passthrough"), AuthType::Passthrough);
        assert_eq!(AuthType::from_str("PASSTHROUGH"), AuthType::Passthrough);
        // Unknown values default to Passthrough (safe for OAuth)
        assert_eq!(AuthType::from_str("unknown"), AuthType::Passthrough);
        assert_eq!(AuthType::from_str(""), AuthType::Passthrough);
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
        };

        assert!(matches!(
            backend.resolve_credential(),
            CredentialStatus::NoAuth
        ));
        assert!(backend.is_configured());
    }
}
