use hyper::{Request, Response, Method};
use hyper::body::Incoming;
use http_body_util::Full;
use hyper::body::Bytes;
use std::sync::Arc;
use anyhow::Result;

use crate::proxy::health::HealthHandler;
use crate::proxy::upstream::UpstreamClient;

#[derive(Clone)]
pub struct RouterEngine {
    health: Arc<HealthHandler>,
    upstream: Arc<UpstreamClient>,
}

impl RouterEngine {
    pub fn new() -> Self {
        Self {
            health: Arc::new(HealthHandler::new()),
            upstream: Arc::new(UpstreamClient::new()),
        }
    }

    pub async fn route(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>> {
        tracing::debug!(method = %req.method(), path = %req.uri().path(), "Incoming request");
        let path = req.uri().path();

        match (req.method(), path) {
            (&Method::GET, "/health") => self.health.handle().await,
            _ => self.upstream.forward(req).await,
        }
    }
}

impl Default for RouterEngine {
    fn default() -> Self {
        Self::new()
    }
}
