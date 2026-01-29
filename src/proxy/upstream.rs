use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, HOST};
use axum::http::{Request, Response};
use http_body_util::BodyExt;
use reqwest::Client;
use crate::backend::BackendState;
use crate::config::build_auth_header;
use crate::proxy::error::ProxyError;
use crate::proxy::timeout::TimeoutConfig;

pub struct UpstreamClient {
    client: Client,
    timeout_config: TimeoutConfig,
}

impl UpstreamClient {
    pub fn new(timeout_config: TimeoutConfig) -> Self {
        let client = Client::builder()
            .connect_timeout(timeout_config.connect)
            .build()
            .expect("Failed to build upstream client");

        Self {
            client,
            timeout_config,
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
        let method = req.method().clone();
        let uri = req.uri();
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
        let mut builder = self.client.request(method, upstream_uri);

        for (name, value) in req.headers() {
            if name != HOST {
                builder = builder.header(name, value);
            }
        }

        if let Some((name, value)) = build_auth_header(&backend) {
            builder = builder.header(&name, value);
        }

        let body_bytes = req
            .into_body()
            .collect()
            .await
            .map_err(|e| ProxyError::InvalidRequest(format!("Failed to read request body: {}", e)))?
            .to_bytes();

        let upstream_resp = builder
            .timeout(self.timeout_config.request)
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ProxyError::RequestTimeout {
                        duration: self.timeout_config.request.as_secs(),
                    }
                } else {
                    ProxyError::ConnectionError {
                        backend: backend.name.clone(),
                        source: e,
                    }
                }
            })?;

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
        Self::new(TimeoutConfig::default())
    }
}
