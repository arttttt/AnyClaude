//! Stage 7: Handle upstream response.
//!
//! Processes the upstream response and converts it to an Axum response:
//! - Detects streaming vs non-streaming
//! - For streaming: creates ObservedStream with callbacks
//! - For non-streaming: reads full body, applies thinking registration
//! - Applies reverse model mapping if needed
//! - Handles debug logging and observability

use axum::body::Body;
use axum::http::header::CONTENT_LENGTH;
use axum::http::Response;

use crate::config::Backend;
use crate::config::DebugLogLevel;
use crate::metrics::{ObservedStream, redact_body, redact_headers, ResponseMeta, ResponsePreview};
use crate::proxy::error::ProxyError;
use crate::proxy::model_rewrite::{make_reverse_model_rewriter, ModelMapping, reverse_model_in_response};
use crate::proxy::thinking::ThinkingSession;
use crate::proxy::pipeline::{PipelineConfig, PipelineContext};

/// Stage 7: Handle upstream response.
///
/// Converts the upstream response into an Axum response, handling both
/// streaming and non-streaming cases.
pub async fn handle_response(
    upstream_resp: reqwest::Response,
    backend: Backend,
    thinking: Option<ThinkingSession>,
    model_mapping: Option<ModelMapping>,
    config: &PipelineConfig,
    ctx: &mut PipelineContext,
) -> Result<Response<Body>, ProxyError> {
    let content_type = upstream_resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    let is_streaming = content_type
        .as_deref()
        .is_some_and(|ct| ct.contains("text/event-stream"));

    let status = upstream_resp.status();
    let response_headers = upstream_resp.headers().clone();

    ctx.span.set_status(status.as_u16());

    // Debug logging for response headers
    let debug_config = ctx.debug_logger.config();
    if debug_config.level >= DebugLogLevel::Full && debug_config.header_preview {
        let meta = ctx
            .span
            .record_mut()
            .response_meta
            .get_or_insert(ResponseMeta {
                headers: None,
                body_preview: None,
            });
        meta.headers = Some(redact_headers(&response_headers));
    }

    // Log error responses for debugging
    if !status.is_success() {
        crate::metrics::app_log(
            "upstream",
            &format!(
                "Upstream returned error status: backend='{}', status={}, content_type={:?}",
                backend.name,
                status,
                content_type.as_deref()
            ),
        );
    }

    let mut response_builder = Response::builder().status(status);

    // Copy response headers, stripping Content-Length if model mapping is active
    for (name, value) in response_headers.iter() {
        if model_mapping.is_some() && name == CONTENT_LENGTH {
            continue;
        }
        response_builder = response_builder.header(name, value);
    }

    if is_streaming {
        // Streaming response path
        let stream = upstream_resp.bytes_stream();

        let response_preview = if debug_config.level >= DebugLogLevel::Full {
            let ct = content_type.clone().unwrap_or_default();
            if debug_config.full_body {
                Some(ResponsePreview::full(ct, debug_config.pretty_print))
            } else {
                ResponsePreview::new(debug_config.body_preview_bytes, ct)
            }
        } else {
            None
        };

        // Register thinking blocks from SSE stream (main agent only)
        let on_complete = thinking.map(|session| {
            Box::new(move |bytes: &[u8]| {
                let events = crate::sse::parse_sse_events(bytes);
                session.register_from_sse(&events);
            }) as crate::metrics::ResponseCompleteCallback
        });

        let mut observed = ObservedStream::new(
            stream,
            ctx.span.clone(),
            ctx.observability.clone(),
            config.timeout_config.idle,
            response_preview,
        );

        if let Some(cb) = on_complete {
            observed = observed.with_on_complete(cb);
        }

        // Reverse model mapping: rewrite model in message_start back to original
        if let Some(mapping) = model_mapping {
            observed = observed.with_chunk_rewriter(make_reverse_model_rewriter(mapping));
        }

        Ok(response_builder.body(Body::from_stream(observed))?)
    } else {
        // Non-streaming response path
        ctx.span.mark_first_byte();
        let body_bytes = match upstream_resp.bytes().await {
            Ok(bytes) => bytes,
            Err(e) => {
                let err = ProxyError::Internal(format!("Failed to read response body: {}", e));
                ctx.observability.finish_error(ctx.span.clone(), Some(err.status_code().as_u16()));
                ctx.span_finalized = true;
                return Err(err);
            }
        };

        // Register thinking blocks from non-streaming response (main agent only)
        if let Some(ref session) = thinking {
            session.register_from_response(&body_bytes);
        }

        // Response analysis for verbose logging
        if debug_config.level >= DebugLogLevel::Verbose {
            use crate::metrics::ResponseParser;
            let parser = ResponseParser::new();
            let mut analysis = parser.parse_response(&body_bytes);
            if analysis.cost_usd.is_none() {
                analysis.cost_usd = compute_cost_usd(
                    &backend,
                    analysis.input_tokens.or_else(|| {
                        ctx.span
                            .record_mut()
                            .request_analysis
                            .as_ref()
                            .and_then(|a| a.estimated_input_tokens)
                    }),
                    analysis.output_tokens,
                );
            }
            ctx.span.record_mut().response_analysis = Some(analysis);
        }

        // Response body preview for full logging
        if debug_config.level >= DebugLogLevel::Full {
            let meta = ctx
                .span
                .record_mut()
                .response_meta
                .get_or_insert(ResponseMeta {
                    headers: None,
                    body_preview: None,
                });
            let limit = if debug_config.full_body {
                None
            } else {
                Some(debug_config.body_preview_bytes)
            };
            meta.body_preview = redact_body(
                &body_bytes,
                content_type.as_deref().unwrap_or(""),
                limit,
                debug_config.pretty_print,
            );
        }

        // Log error response body for debugging
        if !status.is_success() {
            let body_preview = String::from_utf8_lossy(&body_bytes);
            let truncated = if body_preview.len() > 1000 {
                format!("{}...[truncated]", &body_preview[..1000])
            } else {
                body_preview.to_string()
            };
            crate::metrics::app_log(
                "upstream",
                &format!(
                    "Upstream error response body: backend='{}', status={}, body={}",
                    backend.name, status, truncated
                ),
            );
        }

        // Reverse model mapping for non-streaming responses
        let body_bytes = if let Some(ref mapping) = model_mapping {
            reverse_model_in_response(&body_bytes, mapping)
        } else {
            body_bytes
        };

        ctx.span.add_response_bytes(body_bytes.len());
        ctx.observability.finish_request(ctx.span.clone());
        ctx.span_finalized = true;

        Ok(response_builder.body(Body::from(body_bytes))?)
    }
}

fn compute_cost_usd(
    backend: &Backend,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
) -> Option<f64> {
    let pricing = backend.pricing.as_ref()?;
    let input_tokens = input_tokens.unwrap_or(0) as f64;
    let output_tokens = output_tokens.unwrap_or(0) as f64;
    let cost = (input_tokens * pricing.input_per_million + output_tokens * pricing.output_per_million)
        / 1_000_000.0;
    Some(cost)
}
