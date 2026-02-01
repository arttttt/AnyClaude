use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HOST};
use axum::http::{Request, Response};
use http_body_util::BodyExt;
use reqwest::Client;
use tokio::time::sleep;
use crate::backend::BackendState;
use crate::config::{build_auth_header, DebugLogLevel};
use crate::config::ConfigStore;
use crate::metrics::{
    redact_body_preview, redact_headers, DebugLogger, ObservedStream, ObservabilityHub,
    RequestMeta, RequestParser, RequestSpan, ResponseMeta, ResponseParser, ResponsePreview,
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
    config: ConfigStore,
    transformer_registry: Arc<TransformerRegistry>,
    debug_logger: Arc<DebugLogger>,
    request_parser: RequestParser,
    response_parser: ResponseParser,
}

impl UpstreamClient {
    pub fn new(
        timeout_config: TimeoutConfig,
        pool_config: PoolConfig,
        config: ConfigStore,
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
            config,
            transformer_registry,
            debug_logger,
            request_parser: RequestParser::new(true),
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
        let mut body_bytes = body_bytes.to_vec();
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
            meta.body_preview = redact_body_preview(
                &body_bytes,
                request_content_type,
                debug_config.body_preview_bytes,
            );
        }

        if request_content_type.contains("application/json") {
            // Update transformer mode if config changed
            self.transformer_registry
                .update_mode(self.config.get().thinking.mode.clone())
                .await;

            // Parse JSON body for transformation
            if let Ok(mut json_body) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                let context = TransformContext::new(
                    backend.name.clone(),
                    span.request_id().to_string(),
                );

                // Get transformer and apply transformation
                let transformer = self.transformer_registry.get().await;

                match transformer.transform_request(&mut json_body, &context).await {
                    Ok(result) => {
                        if result.changed {
                            // Serialize back to bytes
                            match serde_json::to_vec(&json_body) {
                                Ok(updated) => {
                                    body_bytes = updated;
                                    tracing::info!(
                                        backend = %backend.name,
                                        transformer = transformer.name(),
                                        stripped = result.stats.stripped_count,
                                        "Transformed thinking blocks"
                                    );
                                }
                                Err(e) => {
                                    tracing::error!(
                                        error = %e,
                                        "Failed to serialize transformed request body"
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "Failed to transform thinking blocks"
                        );
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
                if strip_auth_headers {
                    if name == AUTHORIZATION || name.as_str().eq_ignore_ascii_case("x-api-key") {
                        tracing::debug!(
                            header = %name,
                            backend = %backend.name,
                            auth_type = ?backend.auth_type(),
                            "Stripping incoming auth header"
                        );
                        continue;
                    }
                }
                builder = builder.header(name, value);
            }

            // Add backend's own auth header (for bearer/api_key modes)
            if let Some((name, value)) = auth_header.as_ref() {
                builder = builder.header(name, value);
            }

            let send_result = builder
                .timeout(self.timeout_config.request)
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
            .map_or(false, |ct| ct.contains("text/event-stream"));

        let status = upstream_resp.status();
        span.set_status(status.as_u16());

        if debug_level >= DebugLogLevel::Full && debug_config.header_preview {
            let meta = span
                .record_mut()
                .response_meta
                .get_or_insert_with(|| ResponseMeta {
                    headers: None,
                    body_preview: None,
                });
            meta.headers = Some(redact_headers(upstream_resp.headers()));
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

        for (name, value) in upstream_resp.headers() {
            response_builder = response_builder.header(name, value);
        }

        if is_streaming {
            let stream = upstream_resp.bytes_stream();
            let response_preview = if debug_level >= DebugLogLevel::Full {
                ResponsePreview::new(
                    debug_config.body_preview_bytes,
                    content_type.clone().unwrap_or_default(),
                )
            } else {
                None
            };
            let observed = ObservedStream::new(
                stream,
                span,
                observability,
                self.timeout_config.idle,
                response_preview,
            );
            Ok(response_builder.body(Body::from_stream(observed))?)
        } else {
            span.mark_first_byte();
            let body_bytes = match upstream_resp.bytes().await {
                Ok(bytes) => bytes,
                Err(e) => {
                    let err = ProxyError::Internal(format!("Failed to read response body: {}", e));
                    observability.finish_error(span, Some(err.status_code().as_u16()));
                    return Err(err);
                }
            };

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
                    .get_or_insert_with(|| ResponseMeta {
                        headers: None,
                        body_preview: None,
                    });
                meta.body_preview = redact_body_preview(
                    &body_bytes,
                    content_type.as_deref().unwrap_or(""),
                    debug_config.body_preview_bytes,
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

impl Default for UpstreamClient {
    fn default() -> Self {
        let config = ConfigStore::new(
            crate::config::Config::default(),
            crate::config::Config::config_path(),
        );
        let registry = Arc::new(TransformerRegistry::new(
            crate::config::ThinkingMode::Strip,
        ));
        let debug_logger = Arc::new(DebugLogger::new(config.get().debug_logging.clone()));
        Self::new(
            TimeoutConfig::default(),
            PoolConfig::default(),
            config,
            registry,
            debug_logger,
        )
    }
}
