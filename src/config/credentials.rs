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
    pub fn parse(s: &str) -> Self {
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
        AuthType::parse(&self.auth_type_str)
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

    /// Whether to convert adaptive thinking to standard "enabled" format.
    ///
    /// Only enabled when explicitly set to `true` in config. Default: false.
    pub fn needs_thinking_compat(&self) -> bool {
        self.thinking_compat.unwrap_or(false)
    }

}
