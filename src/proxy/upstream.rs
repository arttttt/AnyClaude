use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, HOST};
use axum::http::{Request, Response};
use http_body_util::BodyExt;
use reqwest::Client;
use tokio::time::sleep;
use crate::backend::BackendState;
use crate::config::build_auth_header;
use crate::proxy::error::ProxyError;
use crate::proxy::pool::PoolConfig;
use crate::proxy::timeout::TimeoutConfig;

pub struct UpstreamClient {
    client: Client,
    timeout_config: TimeoutConfig,
    pool_config: PoolConfig,
}

impl UpstreamClient {
    pub fn new(timeout_config: TimeoutConfig, pool_config: PoolConfig) -> Self {
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
        }
    }

    pub async fn forward(
        &self,
        req: Request<Body>,
        backend_state: &BackendState,
    ) -> Result<Response<Body>, ProxyError> {
        // Get the current active backend configuration at request time
        // This ensures the entire request uses the same backend, even if
        // a switch happens mid-request
        let backend = backend_state
            .get_active_backend_config()
            .map_err(|e| ProxyError::BackendNotFound {
                backend: e.to_string(),
            })?;

        self.do_forward(req, backend).await
    }

    async fn do_forward(
        &self,
        req: Request<Body>,
        backend: crate::config::Backend,
    ) -> Result<Response<Body>, ProxyError> {
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
            return Err(ProxyError::BackendNotConfigured {
                backend: backend.name.clone(),
                reason: format!("Environment variable {} not set", backend.auth_env_var),
            });
        }

        let upstream_uri = format!("{}{}", backend.base_url, path_and_query);
        let body_bytes = body
            .collect()
            .await
            .map_err(|e| ProxyError::InvalidRequest(format!("Failed to read request body: {}", e)))?
            .to_bytes();
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
                        return Err(ProxyError::RequestTimeout {
                            duration: self.timeout_config.request.as_secs(),
                        });
                    }

                    return Err(ProxyError::ConnectionError {
                        backend: backend.name.clone(),
                        source: err,
                    });
                }
            }
        };

        let content_type = upstream_resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok());

        let is_streaming = content_type.map_or(false, |ct| ct.contains("text/event-stream"));

        let status = upstream_resp.status();
        let mut response_builder = Response::builder().status(status);

        for (name, value) in upstream_resp.headers() {
            response_builder = response_builder.header(name, value);
        }

        if is_streaming {
            let stream = upstream_resp.bytes_stream();
            Ok(response_builder.body(Body::from_stream(stream))?)
        } else {
            let body_bytes = upstream_resp
                .bytes()
                .await
                .map_err(|e| ProxyError::Internal(format!("Failed to read response body: {}", e)))?;
            Ok(response_builder.body(Body::from(body_bytes))?)
        }
    }
}

impl Default for UpstreamClient {
    fn default() -> Self {
        Self::new(TimeoutConfig::default(), PoolConfig::default())
    }
}
