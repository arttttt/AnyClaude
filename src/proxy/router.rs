use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::Request;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use uuid::Uuid;

use crate::backend::BackendState;
use crate::config::ConfigStore;
use crate::proxy::error::ErrorResponse;
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
}

impl RouterEngine {
    pub fn new(
        config: ConfigStore,
        timeout_config: TimeoutConfig,
        pool_config: PoolConfig,
        backend_state: BackendState,
    ) -> Self {
        Self {
            health: Arc::new(HealthHandler::new()),
            upstream: Arc::new(UpstreamClient::new(timeout_config, pool_config)),
            config,
            backend_state,
        }
    }
}

pub fn build_router(engine: RouterEngine) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .fallback(proxy_handler)
        .with_state(engine)
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

    match state.upstream.forward(req, &state.backend_state).await {
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
