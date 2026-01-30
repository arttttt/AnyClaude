use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{header, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use uuid::Uuid;

use crate::backend::BackendState;
use crate::config::ConfigStore;
use crate::proxy::error::ErrorResponse;
use crate::metrics::{BackendOverride, ObservabilityHub, RoutingDecision};
use crate::proxy::health::HealthHandler;
use crate::proxy::pool::PoolConfig;
use crate::proxy::timeout::TimeoutConfig;
use crate::proxy::upstream::UpstreamClient;

#[derive(Clone)]
pub struct RouterEngine {
    health: Arc<HealthHandler>,
    upstream: Arc<UpstreamClient>,
    #[allow(dead_code)]
    config: ConfigStore,
    backend_state: BackendState,
    observability: ObservabilityHub,
    session_token: String,
}

impl RouterEngine {
    pub fn new(
        config: ConfigStore,
        timeout_config: TimeoutConfig,
        pool_config: PoolConfig,
        backend_state: BackendState,
        observability: ObservabilityHub,
        session_token: String,
    ) -> Self {
        Self {
            health: Arc::new(HealthHandler::new()),
            upstream: Arc::new(UpstreamClient::new(timeout_config, pool_config)),
            config,
            backend_state,
            observability,
            session_token,
        }
    }

    fn is_authorized(&self, req: &Request<Body>) -> bool {
        let Some(value) = req.headers().get(header::AUTHORIZATION) else {
            return false;
        };
        let Ok(value) = value.to_str() else {
            return false;
        };
        let Some(token) = value.strip_prefix("Bearer ") else {
            return false;
        };
        token == self.session_token
    }
}

pub fn build_router(engine: RouterEngine) -> Router {
    let auth_engine = engine.clone();
    Router::new()
        .route("/health", get(health_handler))
        .fallback(proxy_handler)
        .with_state(engine)
        .layer(middleware::from_fn_with_state(auth_engine, auth_middleware))
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
    tracing::debug!(
        method = %req.method(),
        path = %req.uri().path(),
        query = %query_str,
        request_id = %request_id,
        "Incoming request"
    );

    let active_backend = state.backend_state.get_active_backend();
    let mut start = state
        .observability
        .start_request(request_id.clone(), &req, &active_backend);

    let backend_override = start
        .backend_override
        .as_ref()
        .map(|override_backend| override_backend.backend.clone());

    if let Some(BackendOverride { backend, reason }) = start.backend_override.take() {
        start.span.set_backend(backend.clone());
        start.span.record_mut().routing_decision = Some(RoutingDecision {
            backend,
            reason,
        });
    }

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
            tracing::error!(
                request_id = %request_id,
                error = %e,
                error_type = %e.error_type(),
                "Request failed"
            );

            ErrorResponse::from_error(&e, &request_id)
        }
    }
}

async fn auth_middleware(
    State(state): State<RouterEngine>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if !state.is_authorized(&req) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    next.run(req).await
}
