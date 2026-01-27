use hyper::{Response};
use hyper::header::CONTENT_TYPE;
use hyper::body::Bytes;
use http_body_util::Full;
use serde::Serialize;
use anyhow::Result;

#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub status: String,
    pub service: String,
}

pub struct HealthHandler;

impl HealthHandler {
    pub fn new() -> Self {
        Self
    }

    pub async fn handle(&self) -> Result<Response<Full<Bytes>>> {
        let health = HealthStatus {
            status: "healthy".to_string(),
            service: "claudewrapper".to_string(),
        };

        let json = serde_json::to_string(&health).unwrap_or_default();

        Ok(Response::builder()
            .status(200)
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(json)))
            .unwrap())
    }
}

impl Default for HealthHandler {
    fn default() -> Self {
        Self::new()
    }
}
