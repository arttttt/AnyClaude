use hyper::{Request, Response, Method, StatusCode};
use hyper::body::Incoming;
use hyper::body::Bytes;
use std::sync::Arc;

use crate::proxy::health::HealthHandler;

#[derive(Clone)]
pub struct RouterEngine {
    health: Arc<HealthHandler>,
}

impl RouterEngine {
    pub fn new() -> Self {
        Self {
            health: Arc::new(HealthHandler::new()),
        }
    }

    pub async fn route(&self, req: Request<Incoming>) -> Result<Response<Bytes>, hyper::Error> {
        tracing::debug!(method = %req.method(), path = %req.uri().path(), "Incoming request");
        let path = req.uri().path();

        match (req.method(), path) {
            (&Method::GET, "/health") => self.health.handle().await,
            _ => Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Bytes::copy_from_slice(b"Not Found"))
                .unwrap()),
        }
    }
}

impl Default for RouterEngine {
    fn default() -> Self {
        Self::new()
    }
}
