use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::Extension;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use uuid::Uuid;

use crate::backend::{BackendState, SubagentBackend};
use crate::config::{AgentsConfig, DebugLogLevel};
use crate::proxy::error::ErrorResponse;
use crate::metrics::{DebugLogger, ObservabilityHub, RequestMeta};
use crate::proxy::health::HealthHandler;
use crate::proxy::pipeline::{PipelineConfig, PipelineContext};
use crate::proxy::pool::PoolConfig;
use crate::proxy::thinking::TransformerRegistry;
use crate::proxy::timeout::TimeoutConfig;

/// Fixed backend override for the teammate pipeline.
///
/// Set as an axum `Extension` at router build time via `nest("/teammate", ...)`.
/// Extracted by `proxy_handler` to bypass dynamic backend selection.
/// Internal to the routing layer — not part of the public API.
#[derive(Clone)]
pub struct BackendOverride(pub String);

#[derive(Clone)]
pub struct RouterEngine {
    health: Arc<HealthHandler>,
    pub(crate) backend_state: BackendState,
    pub(crate) subagent_backend: SubagentBackend,
    observability: ObservabilityHub,
    pub(crate) debug_logger: Arc<DebugLogger>,
    pipeline_config: Option<PipelineConfig>,
    pub(crate) session_token: Option<String>,
}

impl RouterEngine {
    pub fn new(
        timeout_config: TimeoutConfig,
        pool_config: PoolConfig,
        backend_state: BackendState,
        subagent_backend: SubagentBackend,
        observability: ObservabilityHub,
        debug_logger: Arc<DebugLogger>,
        transformer_registry: Arc<TransformerRegistry>,
        session_token: Option<String>,
    ) -> Self {
        let pipeline_config = Some(PipelineConfig::new(
            backend_state.clone(),
            transformer_registry.clone(),
            timeout_config,
            pool_config,
        ));

        Self {
            health: Arc::new(HealthHandler::new()),
            backend_state,
            subagent_backend,
            observability,
            debug_logger,
            pipeline_config,
            session_token,
        }
    }
}

/// Auth middleware — validates session token for proxy requests.
///
/// Rejects requests without valid x-session-token header when session_token is configured.
async fn auth_middleware(
    State(state): State<RouterEngine>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(ref expected_token) = state.session_token {
        let session_header = req.headers()
            .get("x-session-token")
            .and_then(|v| v.to_str().ok());

        let valid = session_header.map_or(false, |t| t == expected_token);

        if !valid {
            return Response::builder()
                .status(401)
                .body(Body::from("Unauthorized: invalid session token"))
                .unwrap();
        }
    }
    next.run(req).await
}

pub fn build_router(
    engine: RouterEngine,
    agents: &Option<AgentsConfig>,
) -> Router {
    // Main pipeline: auth middleware only (thinking is handled inside the pipeline)
    let main = Router::new()
        .fallback(proxy_handler)
        .layer(axum::middleware::from_fn_with_state(
            engine.clone(),
            auth_middleware,
        ))
        .with_state(engine.clone());

    // Hook endpoints don't need auth — they're called by CC hooks via localhost curl.
    let hook_routes = Router::new()
        .route("/api/subagent-start", post(crate::proxy::hooks::handle_subagent_start))
        .route("/api/subagent-stop", post(crate::proxy::hooks::handle_subagent_stop))
        .with_state(engine.subagent_backend.clone());

    let mut router = Router::new()
        .route("/health", get(health_handler))
        .with_state(engine.clone())
        .merge(hook_routes);

    // TODO: restore auth_middleware for teammate pipeline once session token
    //        is reliably propagated to teammate processes (e.g. via env or CLI flag
    //        instead of tmux shim injection).
    // Teammate pipeline: no session auth (token injection into tmux is unreliable), fixed backend
    if let Some(config) = agents {
        let teammate = Router::new()
            .fallback(proxy_handler)
            .layer(Extension(BackendOverride(
                config.teammate_backend.clone(),
            )))
            .with_state(engine.clone());

        crate::metrics::app_log(
            "router",
            &format!(
                "Teammate pipeline: /teammate/* → backend={}",
                config.teammate_backend,
            ),
        );

        router = router.nest("/teammate", teammate);
    }

    router.merge(main)
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
    use crate::proxy::pipeline::execute_pipeline;

    let request_id = Uuid::new_v4().to_string();
    let query_str = query.as_deref().unwrap_or("");
    crate::metrics::app_log("router", &format!("Incoming request: {} {} request_id={}", req.method(), req.uri().path(), request_id));

    // Backend: from BackendOverride (teammate pipeline) or active backend (main pipeline)
    let teammate_backend = req.extensions()
        .get::<BackendOverride>()
        .map(|bo| bo.0.clone());

    let active_backend = teammate_backend
        .clone()
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

    let backend_override = teammate_backend;

    let pipeline_config = match &state.pipeline_config {
        Some(config) => config.clone(),
        None => {
            let err = crate::proxy::error::ProxyError::Internal(
                "Pipeline config not initialized".to_string()
            );
            crate::metrics::app_log_error("router", &format!("Request failed: request_id={}", request_id), &format!("{} ({})", err, err.error_type()));
            return ErrorResponse::from_error(&err, &request_id);
        }
    };

    let mut pipeline_ctx = PipelineContext::new(
        start.span,
        state.observability.clone(),
        state.debug_logger.clone(),
    );

    match execute_pipeline(req, &pipeline_config, &mut pipeline_ctx, backend_override, start.backend_override).await {
        Ok(resp) => resp,
        Err(e) => {
            crate::metrics::app_log_error("router", &format!("Request failed: request_id={}", request_id), &format!("{} ({})", e, e.error_type()));
            ErrorResponse::from_error(&e, &request_id)
        }
    }
}
