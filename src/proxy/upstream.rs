use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, HOST};
use axum::http::{Request, Response};
use http_body_util::BodyExt;
use reqwest::Client;
use tokio::time::sleep;
use crate::backend::BackendState;
use crate::config::build_auth_header;
use crate::config::ConfigStore;
use crate::metrics::{ObservedStream, ObservabilityHub, RequestSpan};
use crate::proxy::error::ProxyError;
use crate::proxy::pool::PoolConfig;
use crate::proxy::thinking::ThinkingTracker;
use crate::proxy::timeout::TimeoutConfig;
use std::sync::{Arc, RwLock};

pub struct UpstreamClient {
    client: Client,
    timeout_config: TimeoutConfig,
    pool_config: PoolConfig,
    config: ConfigStore,
    thinking_tracker: Arc<RwLock<ThinkingTracker>>,
}

impl UpstreamClient {
    pub fn new(
        timeout_config: TimeoutConfig,
        pool_config: PoolConfig,
        config: ConfigStore,
        thinking_tracker: Arc<RwLock<ThinkingTracker>>,
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
            thinking_tracker,
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

        if request_content_type.contains("application/json") {
            let mut tracker = self
                .thinking_tracker
                .write()
                .expect("thinking tracker lock poisoned");
            tracker.set_mode(self.config.get().thinking.mode);
            let output = tracker.transform_request(&backend.name, &body_bytes);
            if output.result.changed {
                if let Some(updated) = output.body {
                    body_bytes = updated;
                }
                tracing::info!(
                    backend = %backend.name,
                    drop_count = output.result.drop_count,
                    convert_count = output.result.convert_count,
                    tag_count = output.result.tag_count,
                    "Applied thinking compatibility transforms"
                );
            }
        }

        span.set_request_bytes(body_bytes.len());
        let auth_header = build_auth_header(&backend);
        let mut attempt = 0u32;

        let upstream_resp = loop {
            let mut builder = self.client.request(method.clone(), &upstream_uri);

            for (name, value) in headers.iter() {
                if name != HOST {
                    builder = builder.header(name, value);
                }
            }

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
            .and_then(|v| v.to_str().ok());

        let is_streaming = content_type.map_or(false, |ct| ct.contains("text/event-stream"));

        let status = upstream_resp.status();
        span.set_status(status.as_u16());
        let mut response_builder = Response::builder().status(status);

        for (name, value) in upstream_resp.headers() {
            response_builder = response_builder.header(name, value);
        }

        if is_streaming {
            let stream = upstream_resp.bytes_stream();
            let observed = ObservedStream::new(stream, span, observability);
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
            span.add_response_bytes(body_bytes.len());
            observability.finish_request(span);
            Ok(response_builder.body(Body::from(body_bytes))?)
        }
    }
}

impl Default for UpstreamClient {
    fn default() -> Self {
        let config = ConfigStore::new(
            crate::config::Config::default(),
            crate::config::Config::config_path(),
        );
        let tracker = Arc::new(RwLock::new(ThinkingTracker::new(
            crate::config::ThinkingMode::DropSignature,
        )));
        Self::new(TimeoutConfig::default(), PoolConfig::default(), config, tracker)
    }
}
