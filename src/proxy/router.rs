use hyper::{Request, Response, Method};
use hyper::body::Incoming;
use hyper::body::Bytes;
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use std::sync::Arc;
use anyhow::Result;
use uuid::Uuid;

use crate::config::ConfigStore;
use crate::proxy::error::ErrorResponse;
use crate::proxy::health::HealthHandler;
use crate::proxy::timeout::TimeoutConfig;
use crate::proxy::upstream::UpstreamClient;

#[derive(Clone)]
pub struct RouterEngine {
    health: Arc<HealthHandler>,
    upstream: Arc<UpstreamClient>,
    config: ConfigStore,
}

impl RouterEngine {
    pub fn new(config: ConfigStore, timeout_config: TimeoutConfig) -> Self {
        Self {
            health: Arc::new(HealthHandler::new()),
            upstream: Arc::new(UpstreamClient::new(timeout_config)),
            config,
        }
    }

    pub async fn route(&self,
        req: Request<Incoming>,
    ) -> Result<Response<UnsyncBoxBody<Bytes, hyper::Error>>> {
        let request_id = Uuid::new_v4().to_string();
        tracing::debug!(
            method = %req.method(),
            path = %req.uri().path(),
            request_id = %request_id,
            "Incoming request"
        );
        
        let path = req.uri().path();

        match (req.method(), path) {
            (&Method::GET, "/health") => self.health.handle().await,
            _ => {
                match self.upstream.forward(req, self.config.get()).await {
                    Ok(resp) => Ok(resp),
                    Err(e) => {
                        tracing::error!(
                            request_id = %request_id,
                            error = %e,
                            error_type = %e.error_type(),
                            "Request failed"
                        );
                        
                        let error_response = ErrorResponse::from_error(&e, &request_id
                        );
                        
                        // Convert Full<Bytes> to UnsyncBoxBody
                        let (parts, body) = error_response.into_parts();
                        let body_bytes = body.collect().await
                            .map_err(|e| anyhow::anyhow!("Failed to collect error body: {}", e))?
                            .to_bytes();
                        let boxed_body = Full::new(body_bytes)
                            .map_err(|never: std::convert::Infallible| match never {})
                            .boxed_unsync();
                        
                        Ok(Response::from_parts(parts, boxed_body))
                    }
                }
            }
        }
    }
}
