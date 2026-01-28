use hyper::{Request, Response};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HOST, CONTENT_TYPE};
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::time::timeout;

use crate::config::build_auth_header;
use crate::config::Config;
use crate::proxy::error::ProxyError;
use crate::proxy::timeout::TimeoutConfig;

pub struct UpstreamClient {
    client: Client<HttpConnector, Full<Bytes>>,
    timeout_config: TimeoutConfig,
}

impl UpstreamClient {
    pub fn new(timeout_config: TimeoutConfig) -> Self {
        let connector = HttpConnector::new();
        let client = Client::builder(TokioExecutor::new()).build(connector);

        Self {
            client,
            timeout_config,
        }
    }

    pub async fn forward(
        &self,
        req: Request<Incoming>,
        config: Config,
    ) -> Result<Response<UnsyncBoxBody<Bytes, hyper::Error>>, ProxyError> {
        let timeout_config = TimeoutConfig::from(&config.defaults);
        
        // Execute the request with timeout
        let result = timeout(timeout_config.request, self.do_forward(req, config)).await;
        
        match result {
            Ok(response) => response,
            Err(_) => Err(ProxyError::RequestTimeout {
                duration: timeout_config.request.as_secs(),
            }),
        }
    }

    async fn do_forward(
        &self,
        req: Request<Incoming>,
        config: Config,
    ) -> Result<Response<UnsyncBoxBody<Bytes, hyper::Error>>, ProxyError> {
        let method = req.method().clone();
        let uri = req.uri();
        let path_and_query = uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");

        let active_backend_name = &config.defaults.active;
        let backend = config
            .backends
            .iter()
            .find(|b| &b.name == active_backend_name)
            .ok_or_else(|| ProxyError::BackendNotFound {
                backend: active_backend_name.clone(),
            })?;

        // Validate backend is configured
        if !backend.is_configured() {
            return Err(ProxyError::BackendNotConfigured {
                backend: backend.name.clone(),
                reason: format!("Environment variable {} not set", backend.auth_env_var),
            });
        }

        let upstream_uri = format!("{}{}", backend.base_url, path_and_query);

        let mut builder = Request::builder().method(method).uri(upstream_uri);

        for (name, value) in req.headers() {
            if name != HOST {
                builder = builder.header(name, value);
            }
        }

        if let Some((name, value)) = build_auth_header(backend) {
            builder = builder.header(&name, value);
        }

        let body_bytes = req
            .into_body()
            .collect()
            .await
            .map_err(|e| ProxyError::InvalidRequest(format!("Failed to read request body: {}", e)))?
            .to_bytes();

        let upstream_req = builder
            .body(Full::new(body_bytes))
            .map_err(|e| ProxyError::InvalidRequest(format!("Failed to build upstream request: {}", e)))?;

        let upstream_resp = self
            .client
            .request(upstream_req)
            .await
            .map_err(|e| ProxyError::ConnectionError {
                backend: backend.name.clone(),
                source: e,
            })?;

        let content_type = upstream_resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok());

        let is_streaming = content_type.map_or(false, |ct| ct.contains("text/event-stream"));

        let status = upstream_resp.status();
        let mut builder = hyper::Response::builder().status(status);

        for (name, value) in upstream_resp.headers() {
            builder = builder.header(name, value);
        }

        if is_streaming {
            Ok(builder.body(upstream_resp.into_body().boxed_unsync())?)
        } else {
            let body_bytes = upstream_resp
                .into_body()
                .collect()
                .await
                .map_err(|e| ProxyError::Internal(format!("Failed to read response body: {}", e)))?
                .to_bytes();
            Ok(builder.body(
                Full::new(body_bytes)
                    .map_err(|never: std::convert::Infallible| match never {})
                    .boxed_unsync(),
            )?)
        }
    }
}

impl Default for UpstreamClient {
    fn default() -> Self {
        Self::new(TimeoutConfig::default())
    }
}
