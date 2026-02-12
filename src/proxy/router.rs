use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::Extension;
use axum::http::Request;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use uuid::Uuid;

use crate::backend::BackendState;
use crate::config::DebugLogLevel;
use crate::proxy::error::ErrorResponse;
use crate::metrics::{DebugLogger, ObservabilityHub, RequestMeta, RoutingDecision};
use crate::proxy::health::HealthHandler;
use crate::proxy::pool::PoolConfig;
use crate::proxy::thinking::TransformerRegistry;
use crate::proxy::timeout::TimeoutConfig;
use crate::proxy::upstream::UpstreamClient;

#[derive(Clone)]
pub struct RouterEngine {
    health: Arc<HealthHandler>,
    upstream: Arc<UpstreamClient>,
    backend_state: BackendState,
    observability: ObservabilityHub,
    debug_logger: Arc<DebugLogger>,
}

impl RouterEngine {
    pub fn new(
        timeout_config: TimeoutConfig,
        pool_config: PoolConfig,
        backend_state: BackendState,
        observability: ObservabilityHub,
        debug_logger: Arc<DebugLogger>,
        transformer_registry: Arc<TransformerRegistry>,
    ) -> Self {
        Self {
            health: Arc::new(HealthHandler::new()),
            upstream: Arc::new(UpstreamClient::new(
                timeout_config,
                pool_config,
                transformer_registry,
                debug_logger.clone(),
            )),
            backend_state,
            observability,
            debug_logger,
        }
    }
}

pub fn build_router(
    engine: RouterEngine,
    routing_rules: Vec<Box<dyn super::routing::RoutingRule>>,
) -> Router {
    let mut router = Router::new()
        .route("/health", get(health_handler))
        .fallback(proxy_handler)
        .with_state(engine);

    if !routing_rules.is_empty() {
        // Extension must be outermost (added last) so it inserts rules
        // into the request BEFORE the middleware extracts them.
        router = router
            .layer(axum::middleware::from_fn(super::routing::routing_middleware))
            .layer(Extension(Arc::new(routing_rules)));
    }

    router
}

async fn health_handler(
    State(state): State<RouterEngine>,
    RawQuery(_query): RawQuery,
) -> Response {
    state.health.handle().await
}

async fn proxy_handler(
    State(state): State<RouterEngine>,
    RawQuery(query): RawQuery,
    req: Request<Body>,
) -> Response {
    let request_id = Uuid::new_v4().to_string();
    let query_str = query.as_deref().unwrap_or("");
    crate::metrics::app_log("router", &format!("Incoming request: {} {} request_id={}", req.method(), req.uri().path(), request_id));

    let routed = req.extensions().get::<super::routing::RoutedTo>().cloned();
    let active_backend = routed
        .as_ref()
        .map(|r| r.backend.clone())
        .unwrap_or_else(|| state.backend_state.get_active_backend());
    let mut start = state
        .observability
        .start_request(request_id.clone(), &req, &active_backend);

    if state.debug_logger.level() != DebugLogLevel::Off {
        start.span.record_mut().request_meta = Some(RequestMeta {
            method: req.method().to_string(),
            path: req.uri().path().to_string(),
            query: if query_str.is_empty() {
                None
            } else {
                Some(query_str.to_string())
            },
            headers: None,
            body_preview: None,
        });
    }

    let backend_override = start.backend_override.take().map(|bo| {
        start.span.set_backend(bo.backend.clone());
        start.span.record_mut().routing_decision = Some(RoutingDecision {
            backend: bo.backend.clone(),
            reason: bo.reason,
        });
        bo.backend
    }).or(routed.map(|r| r.backend));

    match state
        .upstream
        .forward(
            req,
            &state.backend_state,
            backend_override,
            start.span,
            state.observability.clone(),
        )
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            crate::metrics::app_log_error("router", &format!("Request failed: request_id={}", request_id), &format!("{} ({})", e, e.error_type()));

            ErrorResponse::from_error(&e, &request_id)
        }
    }
}
