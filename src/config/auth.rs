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
        (AuthType::Passthrough, _) => None,
        (_, CredentialStatus::Unconfigured { .. }) => None,
        (_, CredentialStatus::NoAuth) => None,
    }
}
