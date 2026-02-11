//! Rule-based request routing middleware for the proxy.
//!
//! Evaluates routing rules against incoming requests and tags matches
//! with a [`RoutedTo`] extension that `proxy_handler` reads to select
//! the backend. When no rule matches, the handler falls back to the
//! active backend — current behavior, zero overhead.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, Uri};
use axum::middleware::Next;
use axum::response::Response;

/// Routing decision placed into request extensions by the middleware.
/// Read by `proxy_handler` to override the default backend.
#[derive(Clone)]
pub struct RoutedTo {
    pub backend: String,
    pub reason: String,
}

/// Result of a routing rule evaluation.
pub struct RouteAction {
    /// Backend name (must exist in `[[backends]]`).
    pub backend: String,
    /// Human-readable reason for logging/metrics.
    pub reason: String,
    /// Path prefix to strip before forwarding.
    pub strip_prefix: Option<String>,
}

/// A routing rule. Rules are evaluated in order; first match wins.
pub trait RoutingRule: Send + Sync {
    fn evaluate(&self, req: &Request<Body>) -> Option<RouteAction>;
}

/// Routes requests whose path starts with a given prefix to a specific backend.
/// Strips the prefix before forwarding.
pub struct PathPrefixRule {
    pub prefix: String,
    pub backend: String,
}

impl RoutingRule for PathPrefixRule {
    fn evaluate(&self, req: &Request<Body>) -> Option<RouteAction> {
        let path = req.uri().path();
        let is_match = path.strip_prefix(self.prefix.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
        if is_match {
            Some(RouteAction {
                backend: self.backend.clone(),
                reason: format!("path prefix {}", self.prefix),
                strip_prefix: Some(self.prefix.clone()),
            })
        } else {
            None
        }
    }
}

/// Axum middleware that evaluates routing rules and tags the request.
pub async fn routing_middleware(
    Extension(rules): Extension<Arc<Vec<Box<dyn RoutingRule>>>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    for rule in rules.iter() {
        if let Some(action) = rule.evaluate(&req) {
            crate::metrics::app_log(
                "routing",
                &format!(
                    "Routed {} {} -> backend={} reason={}",
                    req.method(),
                    req.uri().path(),
                    action.backend,
                    action.reason,
                ),
            );
            if let Some(ref prefix) = action.strip_prefix {
                rewrite_uri(&mut req, prefix);
            }
            req.extensions_mut().insert(RoutedTo {
                backend: action.backend,
                reason: action.reason,
            });
            break;
        }
    }
    next.run(req).await
}

/// Strip a prefix from the request URI path, preserving query string.
fn rewrite_uri(req: &mut Request<Body>, prefix: &str) {
    let uri = req.uri();
    let path = uri.path();
    let new_path = path.strip_prefix(prefix).unwrap_or(path);
    // Ensure the path starts with '/'
    let new_path = if new_path.starts_with('/') {
        new_path.to_string()
    } else {
        format!("/{new_path}")
    };

    let new_uri = if let Some(query) = uri.query() {
        format!("{new_path}?{query}")
    } else {
        new_path
    };

    if let Ok(parsed) = new_uri.parse::<Uri>() {
        *req.uri_mut() = parsed;
    }
}

/// Build routing rules from `AgentTeamsConfig`.
///
/// Returns an empty vec when `agent_teams` is `None` (no middleware applied).
pub fn build_rules(agent_teams: &Option<crate::config::AgentTeamsConfig>) -> Vec<Box<dyn RoutingRule>> {
    let Some(config) = agent_teams else {
        return Vec::new();
    };

    vec![Box::new(PathPrefixRule {
        prefix: "/teammate".to_string(),
        backend: config.teammate_backend.clone(),
    })]
}
