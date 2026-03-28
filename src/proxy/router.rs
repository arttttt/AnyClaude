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

use crate::backend::{BackendState, AgentBackendState, AgentRegistry};
use crate::config::DebugLogLevel;
use crate::proxy::error::ErrorResponse;
use crate::proxy::hooks::HookState;
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

/// Marker extension for the teammate pipeline.
///
/// Set on `/teammate` nested routes. Signals `proxy_handler` to extract
/// the agent_id from the first URL path segment (e.g. `/teammate/{id}/v1/messages`)
/// and look up the backend in the agent registry.
#[derive(Clone)]
pub struct TeammateMarker;

#[derive(Clone)]
pub struct RouterEngine {
    health: Arc<HealthHandler>,
    pub(crate) backend_state: BackendState,
    pub(crate) subagent_backend: AgentBackendState,
    pub(crate) teammate_backend: AgentBackendState,
    observability: ObservabilityHub,
    pub(crate) debug_logger: Arc<DebugLogger>,
    pipeline_config: PipelineConfig,
    pub(crate) session_token: Option<String>,
}

impl RouterEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        timeout_config: TimeoutConfig,
        pool_config: PoolConfig,
        backend_state: BackendState,
        subagent_backend: AgentBackendState,
        teammate_backend: AgentBackendState,
        agent_registry: AgentRegistry,
        observability: ObservabilityHub,
        debug_logger: Arc<DebugLogger>,
        transformer_registry: Arc<TransformerRegistry>,
        session_token: Option<String>,
    ) -> Self {
        let pipeline_config = PipelineConfig::new(
            backend_state.clone(),
            agent_registry,
            transformer_registry.clone(),
            timeout_config,
            pool_config,
        );

        Self {
            health: Arc::new(HealthHandler::new()),
            backend_state,
            subagent_backend,
            teammate_backend,
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

        let valid = session_header.is_some_and(|t| t == expected_token);

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
    let hook_state = HookState {
        backend_state: engine.backend_state.clone(),
        subagent_backend: engine.subagent_backend.clone(),
        teammate_backend: engine.teammate_backend.clone(),
        registry: engine.pipeline_config.agent_registry.clone(),
    };
    let hook_routes = Router::new()
        .route("/api/subagent-start", post(crate::proxy::hooks::handle_subagent_start))
        .route("/api/subagent-stop", post(crate::proxy::hooks::handle_subagent_stop))
        .route("/api/teammate-start", post(crate::proxy::hooks::handle_teammate_start))
        .with_state(hook_state);

    let mut router = Router::new()
        .route("/health", get(health_handler))
        .with_state(engine.clone())
        .merge(hook_routes);

    // Teammate pipeline: dynamic per-teammate backend via agent_id in URL path.
    // URL: /teammate/{agent_id}/v1/messages → agent_id extracted, path stripped.
    // Always enabled — without [agents] config, falls back to active main backend.
    {
        let teammate = Router::new()
            .fallback(proxy_handler)
            .layer(Extension(TeammateMarker))
            .with_state(engine.clone());

        crate::metrics::app_log(
            "router",
            "Teammate pipeline: /teammate/* → dynamic per-agent routing",
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
    mut req: Request<Body>,
) -> Response {
    use crate::proxy::pipeline::execute_pipeline;

    let request_id = Uuid::new_v4().to_string();
    let query_str = query.as_deref().unwrap_or("");
    crate::metrics::app_log("router", &format!("Incoming request: {} {} request_id={}", req.method(), req.uri().path(), request_id));

    // Determine if this is a teammate request and resolve backend override.
    // TeammateMarker: extract agent_id from first path segment → registry lookup.
    // The shim sets ANTHROPIC_BASE_URL=.../teammate/{agent_id}, so after axum
    // strips /teammate, the remaining path is /{agent_id}/v1/messages.
    // We extract the agent_id, look it up in the registry, and strip it from
    // the URI before forwarding. If the first segment is not a registered
    // agent_id, it is left in place (graceful fallback).
    let is_teammate = req.extensions().get::<TeammateMarker>().is_some();
    let teammate_backend = if is_teammate {
        // Extract candidate agent_id from first path segment: /{agent_id}/v1/messages
        let path = req.uri().path();
        let candidate = path.strip_prefix('/')
            .and_then(|rest| rest.split('/').next())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        // Always strip agent_id segment from URI so pipeline sees /v1/messages.
        // The shim embeds the agent_id in the URL path, so it must be removed
        // before forwarding — regardless of whether the registry knows this id.
        if let Some(ref id) = candidate {
            let prefix = format!("/{}", id);
            let new_path = path.strip_prefix(&prefix).unwrap_or(path);
            let new_path = if new_path.is_empty() { "/" } else { new_path };
            let new_uri = if let Some(q) = req.uri().query() {
                format!("{}?{}", new_path, q)
            } else {
                new_path.to_string()
            };
            if let Ok(uri) = new_uri.parse() {
                *req.uri_mut() = uri;
            }
        }

        // Registry lookup determines backend; fallback to teammate backend.
        let resolved = candidate.as_ref()
            .and_then(|id| state.pipeline_config.agent_registry.lookup(id));

        if let Some(backend) = resolved {
            Some(backend)
        } else {
            if let Some(id) = &candidate {
                crate::metrics::app_log("router", &format!(
                    "Teammate '{}' not in registry, using current teammate backend", id
                ));
            }
            state.teammate_backend.get()
        }
    } else {
        req.extensions()
            .get::<BackendOverride>()
            .map(|bo| bo.0.clone())
    };

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

    let pipeline_config = state.pipeline_config.clone();

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
