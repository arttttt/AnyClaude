use std::collections::HashMap;
use std::time::SystemTime;

use axum::body::Body;
use axum::http::Request;

#[derive(Debug, Clone)]
pub struct RequestRecord {
    pub id: String,
    pub started_at: SystemTime,
    pub first_byte_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
    pub latency_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub backend: String,
    pub status: Option<u16>,
    pub timed_out: bool,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub request_analysis: Option<super::RequestAnalysis>,
    pub response_analysis: Option<ResponseAnalysis>,
    pub routing_decision: Option<RoutingDecision>,
}

#[derive(Debug, Clone)]
pub struct ResponseAnalysis {
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct RoutingDecision {
    pub backend: String,
    pub reason: String,
}

#[derive(Debug, Default, Clone)]
pub struct BackendMetrics {
    pub total: u64,
    pub success_2xx: u64,
    pub client_error_4xx: u64,
    pub server_error_5xx: u64,
    pub timeouts: u64,
    pub avg_latency_ms: f64,
    pub avg_ttfb_ms: f64,
    pub p50_latency_ms: Option<u64>,
    pub p95_latency_ms: Option<u64>,
    pub p99_latency_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub generated_at: SystemTime,
    pub per_backend: HashMap<String, BackendMetrics>,
    pub recent: Vec<RequestRecord>,
}

pub struct BackendOverride {
    pub backend: String,
    pub reason: String,
}

pub struct PreRequestContext<'a> {
    pub request_id: &'a str,
    pub request: &'a Request<Body>,
    pub active_backend: &'a str,
    pub record: &'a mut RequestRecord,
}

pub struct PostResponseContext<'a> {
    pub request_id: &'a str,
    pub record: &'a mut RequestRecord,
}
