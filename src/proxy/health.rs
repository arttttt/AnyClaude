use axum::response::Response;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

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

    pub async fn handle(&self) -> Response {
        let health = HealthStatus {
            status: "healthy".to_string(),
            service: "anyclaude".to_string(),
        };

        Json(health).into_response()
    }
}

impl Default for HealthHandler {
    fn default() -> Self {
        Self::new()
    }
}
