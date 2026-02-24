//! 7-stage linear pipeline for proxy request processing.
//!
//! This module implements a unified pipeline architecture behind the `unified-pipeline`
//! feature flag. The pipeline replaces the legacy middleware-based approach with
//! explicit linear stages.

use axum::body::Body;
use axum::http::{Request, Response};
use std::sync::Arc;

use crate::backend::BackendState;
use crate::metrics::{BackendOverride, DebugLogger, ObservabilityHub, RequestSpan};
use crate::proxy::thinking::TransformerRegistry;

mod extract;
mod forward;
mod headers;
mod response;
mod routing;
mod thinking;
mod transform;

pub use extract::extract_request;
pub use forward::forward_with_retry;
pub use headers::build_headers;
pub use response::handle_response;
pub use routing::resolve_backend;
pub use thinking::create_thinking;
pub use transform::transform_body;

/// Context shared across pipeline stages.
///
/// Contains observability and debugging context that is needed
/// throughout the request lifecycle, but NOT the parsed body
/// (which is passed explicitly between stages).
#[derive(Clone)]
pub struct PipelineContext {
    /// The request span for observability
    pub span: RequestSpan,
    /// Observability hub for metrics
    pub observability: ObservabilityHub,
    /// Debug logger for auxiliary logging
    pub debug_logger: Arc<DebugLogger>,
    /// Whether the observability span has been finalized
    /// (finish_request or finish_error already called by a late stage).
    pub(crate) span_finalized: bool,
}

impl PipelineContext {
    pub fn new(span: RequestSpan, observability: ObservabilityHub, debug_logger: Arc<DebugLogger>) -> Self {
        Self {
            span,
            observability,
            debug_logger,
            span_finalized: false,
        }
    }
}

/// Configuration for pipeline execution.
#[derive(Clone)]
pub struct PipelineConfig {
    /// Backend state for resolving backends
    pub backend_state: BackendState,
    /// Transformer registry for thinking session management
    pub transformer_registry: Arc<TransformerRegistry>,
    /// Request timeout configuration
    pub timeout_config: crate::proxy::timeout::TimeoutConfig,
    /// Pool configuration for retries
    pub pool_config: crate::proxy::pool::PoolConfig,
    /// HTTP client for upstream requests
    pub http_client: reqwest::Client,
}

impl PipelineConfig {
    pub fn new(
        backend_state: BackendState,
        transformer_registry: Arc<TransformerRegistry>,
        timeout_config: crate::proxy::timeout::TimeoutConfig,
        pool_config: crate::proxy::pool::PoolConfig,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .connect_timeout(timeout_config.connect)
            .pool_idle_timeout(Some(pool_config.pool_idle_timeout))
            .pool_max_idle_per_host(pool_config.pool_max_idle_per_host)
            .build()
            .expect("Failed to build upstream client");

        Self {
            backend_state,
            transformer_registry,
            timeout_config,
            pool_config,
            http_client,
        }
    }
}

/// Execute the 7-stage pipeline for a single request.
///
/// This is the main entry point for the unified pipeline. It orchestrates
/// all 7 stages in sequence and handles error propagation.
///
/// Observability lifecycle: stages 6-7 call `finish_error`/`finish_request`
/// internally for late errors. For early errors (stages 1-5), this function
/// ensures `finish_error` is called before returning.
pub async fn execute_pipeline(
    req: Request<Body>,
    config: &PipelineConfig,
    ctx: &mut PipelineContext,
    backend_override: Option<String>,
    plugin_override: Option<BackendOverride>,
) -> Result<Response<Body>, crate::proxy::error::ProxyError> {
    let is_teammate = backend_override.is_some();

    match execute_pipeline_inner(req, config, ctx, backend_override, plugin_override, is_teammate).await {
        Ok(response) => Ok(response),
        Err(e) => {
            // Late stages (forward, response) set span_finalized=true when they
            // call finish_error/finish_request. For early errors (stages 1-5),
            // finalize the span here to avoid dangling spans.
            if !ctx.span_finalized {
                ctx.observability.finish_error(ctx.span.clone(), Some(e.status_code().as_u16()));
                ctx.span_finalized = true;
            }
            Err(e)
        }
    }
}

async fn execute_pipeline_inner(
    req: Request<Body>,
    config: &PipelineConfig,
    ctx: &mut PipelineContext,
    backend_override: Option<String>,
    plugin_override: Option<BackendOverride>,
    is_teammate: bool,
) -> Result<Response<Body>, crate::proxy::error::ProxyError> {
    // Stage 1: Extract request
    let extracted = extract::extract_request(req, ctx).await?;

    // Stage 2: Resolve backend
    let backend = routing::resolve_backend(
        &config.backend_state,
        backend_override,
        plugin_override,
        extracted.parsed_body.as_ref(),
        ctx,
    )?;

    // Stage 3: Create thinking session (after routing, before transform)
    // Teammate requests (those with backend_override) skip thinking.
    let thinking_session = if is_teammate {
        None
    } else {
        thinking::create_thinking(
            &config.transformer_registry,
            &backend,
            ctx,
        )
    };

    // Stage 4: Transform body
    let (transformed_body, is_streaming, model_mapping) = transform::transform_body(
        extracted.body_bytes,
        extracted.parsed_body,
        &backend,
        thinking_session.as_ref(),
        ctx,
    )?;

    // Update span with request bytes after transformation
    ctx.span.set_request_bytes(transformed_body.len());

    // Stage 5: Build headers
    let headers = headers::build_headers(
        &extracted.headers,
        &backend,
        ctx,
    )?;

    // Stage 6: Forward with retry
    let upstream_resp = forward::forward_with_retry(
        &config.http_client,
        extracted.method,
        extracted.uri,
        headers,
        transformed_body,
        is_streaming,
        &backend,
        config,
        ctx,
    ).await?;

    // Stage 7: Handle response
    let response = response::handle_response(
        upstream_resp,
        backend,
        thinking_session,
        model_mapping,
        config,
        ctx,
    ).await?;

    Ok(response)
}
