use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HOST};
use axum::http::{Request, Response};
use http_body_util::BodyExt;
use reqwest::Client;
use tokio::time::sleep;
use crate::backend::BackendState;
use crate::config::{build_auth_header, DebugLogLevel};
use crate::metrics::{
    redact_body, redact_headers, DebugLogger, ObservedStream, ObservabilityHub, RequestMeta,
    RequestParser, RequestSpan, ResponseMeta, ResponseParser, ResponsePreview,
};
use crate::proxy::error::ProxyError;
use crate::proxy::pool::PoolConfig;
use crate::proxy::thinking::{TransformContext, TransformerRegistry};
use crate::proxy::timeout::TimeoutConfig;
use std::sync::Arc;

pub struct UpstreamClient {
    client: Client,
    timeout_config: TimeoutConfig,
    pool_config: PoolConfig,
    transformer_registry: Arc<TransformerRegistry>,
    debug_logger: Arc<DebugLogger>,
    request_parser: RequestParser,
    response_parser: ResponseParser,
}

impl UpstreamClient {
    pub fn new(
        timeout_config: TimeoutConfig,
        pool_config: PoolConfig,
        transformer_registry: Arc<TransformerRegistry>,
        debug_logger: Arc<DebugLogger>,
    ) -> Self {
        let client = Client::builder()
            .connect_timeout(timeout_config.connect)
            .pool_idle_timeout(Some(pool_config.pool_idle_timeout))
            .pool_max_idle_per_host(pool_config.pool_max_idle_per_host)
            .build()
            .expect("Failed to build upstream client");

        Self {
            client,
            timeout_config,
            pool_config,
            transformer_registry,
            debug_logger,
            request_parser: RequestParser::new(),
            response_parser: ResponseParser::new(),
        }
    }

    pub async fn forward(
        &self,
        req: Request<Body>,
        backend_state: &BackendState,
        backend_override: Option<String>,
        span: RequestSpan,
        observability: ObservabilityHub,
    ) -> Result<Response<Body>, ProxyError> {
        // Get the current active backend configuration at request time
        // This ensures the entire request uses the same backend, even if
        // a switch happens mid-request
        let backend = match backend_override.as_deref() {
            Some(backend_id) => backend_state
                .get_backend_config(backend_id)
                .map_err(|e| ProxyError::BackendNotFound {
                    backend: e.to_string(),
                }),
            None => backend_state
                .get_active_backend_config()
                .map_err(|e| ProxyError::BackendNotFound {
                    backend: e.to_string(),
                }),
        };

        let backend = match backend {
            Ok(backend) => backend,
            Err(err) => {
                observability.finish_error(span, Some(err.status_code().as_u16()));
                return Err(err);
            }
        };

        self.do_forward(req, backend, span, observability).await
    }

    async fn do_forward(
        &self,
        req: Request<Body>,
        backend: crate::config::Backend,
        mut span: RequestSpan,
        observability: ObservabilityHub,
    ) -> Result<Response<Body>, ProxyError> {
        span.set_backend(backend.name.clone());
        let (parts, body) = req.into_parts();
        let method = parts.method;
        let uri = parts.uri;
        let headers = parts.headers;
        let path_and_query = uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");

        let debug_config = self.debug_logger.config();
        let debug_level = debug_config.level;
        if debug_level >= DebugLogLevel::Full && debug_config.header_preview {
            let record = span.record_mut();
            let meta = record.request_meta.get_or_insert_with(|| {
                let query = uri.query().map(|value| value.to_string());
                RequestMeta {
                    method: method.to_string(),
                    path: uri.path().to_string(),
                    query,
                    headers: None,
                    body_preview: None,
                }
            });
            meta.headers = Some(redact_headers(&headers));
        }

        // Validate backend is configured
        if !backend.is_configured() {
            let err = ProxyError::BackendNotConfigured {
                backend: backend.name.clone(),
                reason: "api_key is not set".to_string(),
            };
            observability.finish_error(span, Some(err.status_code().as_u16()));
            return Err(err);
        }

        let upstream_uri = format!("{}{}", backend.base_url, path_and_query);
        let body_bytes = match body.collect().await {
            Ok(bytes) => bytes.to_bytes(),
            Err(e) => {
                let err = ProxyError::InvalidRequest(format!("Failed to read request body: {}", e));
                observability.finish_error(span, Some(err.status_code().as_u16()));
                return Err(err);
            }
        };
        let body_bytes = body_bytes.to_vec();
        let request_content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if debug_level >= DebugLogLevel::Verbose {
            let analysis = self.request_parser.parse_request(&body_bytes);
            span.record_mut().request_analysis = Some(analysis);
        }

        if debug_level >= DebugLogLevel::Full {
            let meta = span
                .record_mut()
                .request_meta
                .get_or_insert_with(|| {
                    let query = uri.query().map(|value| value.to_string());
                    RequestMeta {
                        method: method.to_string(),
                        path: uri.path().to_string(),
                        query,
                        headers: None,
                        body_preview: None,
                    }
                });
            let limit = if debug_config.full_body {
                None
            } else {
                Some(debug_config.body_preview_bytes)
            };
            meta.body_preview = redact_body(
                &body_bytes,
                request_content_type,
                limit,
                debug_config.pretty_print,
            );
        }

        // Apply thinking transformer
        let mut body_bytes = body_bytes;
        let needs_thinking_compat = backend.needs_thinking_compat();

        // Detect streaming requests: reqwest's .timeout() covers the entire response
        // body read, which kills SSE streams mid-generation. For streaming requests
        // we rely on connect_timeout (Client-level) + idle_timeout (ObservedStream).
        let mut is_streaming_request = false;

        // Notify thinking registry about current backend (increments session on switch)
        self.transformer_registry
            .notify_backend_for_thinking(&backend.name);
        // Capture session ID now — the on_complete callback may fire after a subsequent
        // request has already incremented the session via notify_backend_for_thinking.
        let thinking_session_id = self.transformer_registry.current_thinking_session();

        if request_content_type.contains("application/json") {
            if let Ok(mut json_body) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                is_streaming_request = json_body
                    .get("stream")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Convert adaptive thinking to standard format for non-Anthropic backends
                let mut thinking_converted = false;
                if needs_thinking_compat {
                    if let Some(changed) =
                        convert_adaptive_thinking(&mut json_body, backend.thinking_budget_tokens)
                    {
                        if changed {
                            thinking_converted = true;
                            self.debug_logger.log_auxiliary(
                                "thinking_compat",
                                None,
                                None,
                                Some(&format!(
                                    "Converted adaptive → enabled for backend '{}', budget={}",
                                    backend.name,
                                    json_body.get("thinking")
                                        .and_then(|t| t.get("budget_tokens"))
                                        .and_then(|b| b.as_u64())
                                        .unwrap_or(0)
                                )),
                                None,
                            );
                        }
                    }
                }

                // Filter out thinking blocks from other sessions
                let cache_before = self.transformer_registry.thinking_cache_stats();
                let filtered = self.transformer_registry.filter_thinking_blocks(&mut json_body);
                if filtered > 0 || cache_before.total > 0 {
                    self.debug_logger.log_auxiliary(
                        "thinking_filter",
                        None, None,
                        Some(&format!(
                            "Filter: cache={} blocks, removed={} from request",
                            cache_before.total, filtered,
                        )),
                        None,
                    );
                }

                let context = TransformContext::new(
                    backend.name.clone(),
                    span.request_id().to_string(),
                    path_and_query,
                );

                let transformer = self.transformer_registry.transformer();
                let mut transformer_changed = false;

                match transformer.transform_request(&mut json_body, &context) {
                    Ok(result) => {
                        transformer_changed = result.changed;
                        if result.changed {
                            tracing::info!(
                                backend = %backend.name,
                                transformer = transformer.name(),
                                "Applied transformer"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "Failed to apply transformer"
                        );
                    }
                }

                // Re-serialize body if any transformation occurred
                if thinking_converted || filtered > 0 || transformer_changed {
                    if thinking_converted {
                        let thinking_json = json_body.get("thinking")
                            .map(|t| t.to_string())
                            .unwrap_or_else(|| "null".to_string());
                        self.debug_logger.log_auxiliary(
                            "thinking_compat",
                            None,
                            None,
                            Some(&format!(
                                "Final request thinking field: {}",
                                thinking_json
                            )),
                            None,
                        );
                    }
                    match serde_json::to_vec(&json_body) {
                        Ok(updated) => body_bytes = updated,
                        Err(e) => {
                            tracing::error!(
                                error = %e,
                                "Failed to serialize transformed request body, using original"
                            );
                        }
                    }
                }
            }
        }

        span.set_request_bytes(body_bytes.len());
        let auth_header = build_auth_header(&backend);
        let mut attempt = 0u32;

        let upstream_resp = loop {
            let mut builder = self.client.request(method.clone(), &upstream_uri);

            // Determine if we should strip incoming auth headers based on backend's auth type
            let strip_auth_headers = backend.auth_type().uses_own_credentials();

            for (name, value) in headers.iter() {
                // Always skip HOST and CONTENT_LENGTH - they will be set by the HTTP client
                // CONTENT_LENGTH must be recalculated after body transformation
                if name == HOST || name == CONTENT_LENGTH {
                    continue;
                }
                // Strip auth headers when backend uses its own credentials (bearer/api_key)
                // Passthrough mode forwards all headers unchanged
                if strip_auth_headers
                    && (name == AUTHORIZATION || name.as_str().eq_ignore_ascii_case("x-api-key")) {
                        tracing::debug!(
                            header = %name,
                            backend = %backend.name,
                            auth_type = ?backend.auth_type(),
                            "Stripping incoming auth header"
                        );
                        continue;
                    }
                // Rewrite anthropic-beta header for non-Anthropic backends
                if needs_thinking_compat
                    && name.as_str().eq_ignore_ascii_case("anthropic-beta")
                {
                    if let Ok(val) = value.to_str() {
                        let patched = patch_anthropic_beta_header(val);
                        if patched != val {
                            self.debug_logger.log_auxiliary(
                                "thinking_compat",
                                None,
                                None,
                                Some(&format!(
                                    "Patched anthropic-beta: '{}' → '{}'",
                                    val, patched
                                )),
                                None,
                            );
                        }
                        builder = builder.header(name, &patched);
                        continue;
                    }
                }
                builder = builder.header(name, value);
            }

            // Add backend's own auth header (for bearer/api_key modes)
            if let Some((name, value)) = auth_header.as_ref() {
                builder = builder.header(name, value);
            }

            // For streaming requests: skip reqwest timeout entirely.
            // connect_timeout is set on Client, idle_timeout on ObservedStream.
            // For non-streaming: apply request timeout to the full response.
            if !is_streaming_request {
                builder = builder.timeout(self.timeout_config.request);
            }

            let send_result = builder
                .body(body_bytes.clone())
                .send()
                .await;

            match send_result {
                Ok(response) => break response,
                Err(err) => {
                    tracing::error!(
                        backend = %backend.name,
                        is_connect = err.is_connect(),
                        is_timeout = err.is_timeout(),
                        is_request = err.is_request(),
                        is_body = err.is_body(),
                        error = ?err,
                        "Upstream request error details"
                    );
                    let should_retry = err.is_connect() || err.is_timeout();
                    if should_retry && attempt < self.pool_config.max_retries {
                        let backoff = self
                            .pool_config
                            .retry_backoff_base
                            .saturating_mul(1u32 << attempt);
                        tracing::warn!(
                            backend = %backend.name,
                            attempt = attempt + 1,
                            max_retries = self.pool_config.max_retries,
                            backoff_ms = backoff.as_millis(),
                            error = %err,
                            "Upstream request failed, retrying"
                        );
                        sleep(backoff).await;
                        attempt += 1;
                        continue;
                    }

                    if err.is_timeout() {
                        let timeout_err = ProxyError::RequestTimeout {
                            duration: self.timeout_config.request.as_secs(),
                        };
                        span.mark_timed_out();
                        observability.finish_error(span, Some(timeout_err.status_code().as_u16()));
                        return Err(timeout_err);
                    }

                    let conn_err = ProxyError::ConnectionError {
                        backend: backend.name.clone(),
                        source: err,
                    };
                    observability.finish_error(span, Some(conn_err.status_code().as_u16()));
                    return Err(conn_err);
                }
            }
        };

        let content_type = upstream_resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|value| value.to_string());

        let is_streaming = content_type
            .as_deref()
            .is_some_and(|ct| ct.contains("text/event-stream"));

        let status = upstream_resp.status();
        let response_headers = upstream_resp.headers().clone();

        span.set_status(status.as_u16());

        if debug_level >= DebugLogLevel::Full && debug_config.header_preview {
            let meta = span
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
            tracing::warn!(
                backend = %backend.name,
                status = %status,
                content_type = ?content_type.as_deref(),
                "Upstream returned error status"
            );
        }

        let mut response_builder = Response::builder().status(status);

        for (name, value) in response_headers.iter() {
            response_builder = response_builder.header(name, value);
        }

        if is_streaming {
            let stream = upstream_resp.bytes_stream();
            let response_preview = if debug_level >= DebugLogLevel::Full {
                let ct = content_type.clone().unwrap_or_default();
                if debug_config.full_body {
                    Some(ResponsePreview::full(ct, debug_config.pretty_print))
                } else {
                    ResponsePreview::new(debug_config.body_preview_bytes, ct)
                }
            } else {
                None
            };

            // Create callback to capture response for thinking registration
            let registry = Arc::clone(&self.transformer_registry);
            let cb_debug_logger = Arc::clone(&self.debug_logger);
            let on_complete: crate::metrics::ResponseCompleteCallback = Box::new(move |bytes| {
                // Parse SSE events once, reuse for diagnostics and registration
                let events = crate::sse::parse_sse_events(bytes);
                let thinking_stats = crate::sse::analyze_thinking_stream(&events);
                cb_debug_logger.log_auxiliary(
                    "sse_callback",
                    None, None,
                    Some(&format!(
                        "SSE callback: {} bytes, {} events, thinking: {}",
                        bytes.len(),
                        events.len(),
                        thinking_stats,
                    )),
                    None,
                );

                // Register thinking blocks synchronously to avoid race condition:
                // next request's filter must see these blocks in the registry.
                let before = registry.thinking_cache_stats();
                registry.register_thinking_from_sse_stream(&events, thinking_session_id);
                let after = registry.thinking_cache_stats();
                let registered = after.total.saturating_sub(before.total);
                cb_debug_logger.log_auxiliary(
                    "sse_callback",
                    None, None,
                    Some(&format!(
                        "Registered {} new thinking blocks (cache: {} → {})",
                        registered, before.total, after.total,
                    )),
                    None,
                );

            });

            let observed = ObservedStream::new(
                stream,
                span,
                observability,
                self.timeout_config.idle,
                response_preview,
            )
            .with_on_complete(on_complete);

            Ok(response_builder.body(Body::from_stream(observed))?)
        } else {
            span.mark_first_byte();
            let body_bytes = match upstream_resp.bytes().await {
                Ok(bytes) => bytes,
                Err(e) => {
                    let err =
                        ProxyError::Internal(format!("Failed to read response body: {}", e));
                    observability.finish_error(span, Some(err.status_code().as_u16()));
                    return Err(err);
                }
            };

            // Register thinking blocks from non-streaming response
            self.transformer_registry
                .register_thinking_from_response(&body_bytes, thinking_session_id);

            if debug_level >= DebugLogLevel::Verbose {
                let mut analysis = self.response_parser.parse_response(&body_bytes);
                if analysis.cost_usd.is_none() {
                    analysis.cost_usd = compute_cost_usd(
                        &backend,
                        analysis.input_tokens.or_else(|| {
                            span.record_mut()
                                .request_analysis
                                .as_ref()
                                .and_then(|analysis| analysis.estimated_input_tokens)
                        }),
                        analysis.output_tokens,
                    );
                }
                span.record_mut().response_analysis = Some(analysis);
            }

            if debug_level >= DebugLogLevel::Full {
                let meta = span
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
                tracing::warn!(
                    backend = %backend.name,
                    status = %status,
                    body = %truncated,
                    "Upstream error response body"
                );
            }

            span.add_response_bytes(body_bytes.len());
            observability.finish_request(span);
            Ok(response_builder.body(Body::from(body_bytes))?)
        }
    }
}

fn compute_cost_usd(
    backend: &crate::config::Backend,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
) -> Option<f64> {
    let pricing = backend.pricing.as_ref()?;
    let input_tokens = input_tokens.unwrap_or(0) as f64;
    let output_tokens = output_tokens.unwrap_or(0) as f64;
    let cost = (input_tokens * pricing.input_per_million
        + output_tokens * pricing.output_per_million)
        / 1_000_000.0;
    Some(cost)
}

/// Convert `"thinking": {"type": "adaptive"}` to `"thinking": {"type": "enabled", "budget_tokens": N}`.
///
/// Budget priority: explicit config (`thinking_budget_tokens`) > `max_tokens - 1` from request > default 10000.
///
/// Returns `Some(true)` if converted, `Some(false)` if thinking exists but not adaptive,
/// `None` if no thinking field present.
fn convert_adaptive_thinking(body: &mut serde_json::Value, configured_budget: Option<u32>) -> Option<bool> {
    let is_adaptive = body
        .get("thinking")
        .and_then(|t| t.get("type"))
        .and_then(|t| t.as_str())
        == Some("adaptive");

    if !is_adaptive {
        return body.get("thinking").map(|_| false);
    }

    let budget = configured_budget.unwrap_or_else(|| {
        body.get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|mt| mt.saturating_sub(1) as u32)
            .unwrap_or(10_000)
    });

    body.as_object_mut()?.insert(
        "thinking".to_string(),
        serde_json::json!({
            "type": "enabled",
            "budget_tokens": budget
        }),
    );
    Some(true)
}

/// Rewrite anthropic-beta header for non-Anthropic backends:
/// strip `adaptive-thinking-*` and ensure `interleaved-thinking-2025-05-14` is present.
fn patch_anthropic_beta_header(value: &str) -> String {
    let mut parts: Vec<&str> = value
        .split(',')
        .map(|p| p.trim())
        .filter(|part| !part.starts_with("adaptive-thinking-"))
        .collect();

    let has_interleaved = parts
        .iter()
        .any(|p| p.starts_with("interleaved-thinking-"));
    if !has_interleaved {
        parts.push("interleaved-thinking-2025-05-14");
    }

    parts.join(",")
}

impl Default for UpstreamClient {
    fn default() -> Self {
        let registry = Arc::new(TransformerRegistry::new());
        let debug_logger = Arc::new(DebugLogger::new(Default::default()));
        Self::new(
            TimeoutConfig::default(),
            PoolConfig::default(),
            registry,
            debug_logger,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- convert_adaptive_thinking ---

    #[test]
    fn test_convert_adaptive_to_enabled_with_config_budget() {
        let mut body = serde_json::json!({
            "model": "claude-opus-4-6",
            "max_tokens": 32000,
            "thinking": {"type": "adaptive"}
        });
        let result = convert_adaptive_thinking(&mut body, Some(16000));
        assert_eq!(result, Some(true));
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 16000);
    }

    #[test]
    fn test_convert_adaptive_falls_back_to_max_tokens() {
        let mut body = serde_json::json!({
            "model": "claude-opus-4-6",
            "max_tokens": 32000,
            "thinking": {"type": "adaptive"}
        });
        let result = convert_adaptive_thinking(&mut body, None);
        assert_eq!(result, Some(true));
        assert_eq!(body["thinking"]["budget_tokens"], 31999);
    }

    #[test]
    fn test_convert_adaptive_falls_back_to_default() {
        let mut body = serde_json::json!({
            "model": "claude-opus-4-6",
            "thinking": {"type": "adaptive"}
        });
        let result = convert_adaptive_thinking(&mut body, None);
        assert_eq!(result, Some(true));
        assert_eq!(body["thinking"]["budget_tokens"], 10000);
    }

    #[test]
    fn test_convert_enabled_unchanged() {
        let mut body = serde_json::json!({
            "thinking": {"type": "enabled", "budget_tokens": 8000}
        });
        let result = convert_adaptive_thinking(&mut body, Some(16000));
        assert_eq!(result, Some(false));
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 8000);
    }

    #[test]
    fn test_convert_no_thinking_field() {
        let mut body = serde_json::json!({"model": "claude-opus-4-6"});
        let result = convert_adaptive_thinking(&mut body, Some(16000));
        assert_eq!(result, None);
    }

    // --- patch_anthropic_beta_header ---

    #[test]
    fn test_patch_header_replaces_adaptive() {
        let header = "claude-code-20250219,adaptive-thinking-2026-01-28,prompt-caching-2024-07-31";
        let patched = patch_anthropic_beta_header(header);
        assert!(!patched.contains("adaptive-thinking"));
        assert!(patched.contains("interleaved-thinking-2025-05-14"));
        assert!(patched.contains("claude-code-20250219"));
        assert!(patched.contains("prompt-caching-2024-07-31"));
    }

    #[test]
    fn test_patch_header_preserves_existing_interleaved() {
        let header = "interleaved-thinking-2025-05-14,prompt-caching-2024-07-31";
        let patched = patch_anthropic_beta_header(header);
        assert_eq!(
            patched.matches("interleaved-thinking").count(),
            1,
            "should not duplicate interleaved-thinking"
        );
    }

    #[test]
    fn test_patch_header_no_adaptive_adds_interleaved() {
        let header = "claude-code-20250219,prompt-caching-2024-07-31";
        let patched = patch_anthropic_beta_header(header);
        assert!(patched.contains("interleaved-thinking-2025-05-14"));
    }

    #[test]
    fn test_patch_header_only_adaptive() {
        let header = "adaptive-thinking-2026-01-28";
        let patched = patch_anthropic_beta_header(header);
        assert_eq!(patched, "interleaved-thinking-2025-05-14");
    }
}
