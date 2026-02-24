use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::Extension;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use uuid::Uuid;

use crate::backend::BackendState;
use crate::config::{AgentTeamsConfig, DebugLogLevel};
use crate::proxy::error::ErrorResponse;
use crate::metrics::{DebugLogger, ObservabilityHub, RequestMeta};
#[cfg(not(feature = "unified-pipeline"))]
use crate::metrics::RoutingDecision;
use crate::proxy::health::HealthHandler;
use crate::proxy::pool::PoolConfig;
use crate::proxy::thinking::TransformerRegistry;
use crate::proxy::timeout::TimeoutConfig;
#[cfg(not(feature = "unified-pipeline"))]
use crate::proxy::upstream::UpstreamClient;

#[cfg(feature = "unified-pipeline")]
use crate::proxy::pipeline::{PipelineConfig, PipelineContext};

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
    #[cfg(not(feature = "unified-pipeline"))]
    upstream: Arc<UpstreamClient>,
    pub(crate) backend_state: BackendState,
    observability: ObservabilityHub,
    pub(crate) debug_logger: Arc<DebugLogger>,
    pub(crate) transformer_registry: Arc<TransformerRegistry>,
    #[cfg(feature = "unified-pipeline")]
    pipeline_config: Option<PipelineConfig>,
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
        #[cfg(feature = "unified-pipeline")]
        let pipeline_config = Some(PipelineConfig::new(
            backend_state.clone(),
            transformer_registry.clone(),
            timeout_config,
            pool_config,
        ));

        Self {
            health: Arc::new(HealthHandler::new()),
            #[cfg(not(feature = "unified-pipeline"))]
            upstream: Arc::new(UpstreamClient::new(
                timeout_config,
                pool_config,
                debug_logger.clone(),
            )),
            backend_state,
            observability,
            debug_logger,
            transformer_registry,
            #[cfg(feature = "unified-pipeline")]
            pipeline_config,
        }
    }
}

pub fn build_router(
    engine: RouterEngine,
    teams: &Option<AgentTeamsConfig>,
) -> Router {
    // Main pipeline: with thinking middleware
    let main = Router::new()
        .fallback(proxy_handler)
        .layer(axum::middleware::from_fn_with_state(
            engine.clone(),
            thinking_middleware,
        ))
        .with_state(engine.clone());

    let mut router = Router::new()
        .route("/health", get(health_handler))
        .with_state(engine.clone());

    // Teammate pipeline: no thinking, fixed backend
    if let Some(config) = teams {
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

/// Thinking middleware — creates a ThinkingSession for main agent requests.
///
/// Only present in the main pipeline. Not applied to teammate routes.
async fn thinking_middleware(
    State(state): State<RouterEngine>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let backend = state.backend_state.get_active_backend();
    let session = state.transformer_registry.begin_request(
        &backend,
        state.debug_logger.clone(),
    );
    req.extensions_mut().insert(session);
    next.run(req).await
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

    // Use unified pipeline when feature is enabled
    #[cfg(feature = "unified-pipeline")]
    {
        use crate::proxy::pipeline::execute_pipeline;

        // Pipeline handles all routing internally (plugin, teammate, marker, active)
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

    // Use legacy upstream client when feature is disabled
    #[cfg(not(feature = "unified-pipeline"))]
    {
        // Determine final backend override for forward().
        // Priority: observability plugin > teammate route > none (use active).
        let backend_override = if let Some(obs) = start.backend_override {
            start.span.set_backend(obs.backend.clone());
            start.span.record_mut().routing_decision = Some(RoutingDecision {
                backend: obs.backend.clone(),
                reason: obs.reason,
            });
            Some(obs.backend)
        } else if let Some(teammate) = teammate_backend {
            start.span.set_backend(teammate.clone());
            start.span.record_mut().routing_decision = Some(RoutingDecision {
                backend: teammate.clone(),
                reason: "teammate route".to_string(),
            });
            Some(teammate)
        } else {
            None
        };

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
}
