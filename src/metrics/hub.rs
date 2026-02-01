use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::RwLock;

use axum::body::Body;
use axum::http::Request;

use super::aggregator::{apply_percentiles, BackendAccumulator};
use super::plugin::ObservabilityPlugin;
use super::ring::RequestRingBuffer;
use super::span::{finalize_record, RequestSpan, RequestStart};
use super::types::{
    BackendMetrics, MetricsSnapshot, PostResponseContext, PreRequestContext, RequestRecord,
};

#[derive(Clone)]
pub struct ObservabilityHub {
    inner: Arc<ObservabilityInner>,
}

struct ObservabilityInner {
    ring: RequestRingBuffer,
    aggregates: RwLock<HashMap<String, BackendAccumulator>>,
    plugins: Vec<Arc<dyn ObservabilityPlugin>>,
}

impl ObservabilityHub {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(ObservabilityInner {
                ring: RequestRingBuffer::new(capacity),
                aggregates: RwLock::new(HashMap::new()),
                plugins: Vec::new(),
            }),
        }
    }

    pub fn with_plugins(mut self, plugins: Vec<Arc<dyn ObservabilityPlugin>>) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.plugins = plugins;
        }
        self
    }

    pub fn start_request(
        &self,
        request_id: String,
        request: &Request<Body>,
        active_backend: &str,
    ) -> RequestStart {
        let started_at = SystemTime::now();
        let mut record = RequestRecord {
            id: request_id.clone(),
            started_at,
            first_byte_at: None,
            completed_at: None,
            latency_ms: None,
            ttfb_ms: None,
            backend: active_backend.to_string(),
            status: None,
            timed_out: false,
            request_bytes: 0,
            response_bytes: 0,
            request_analysis: None,
            response_analysis: None,
            routing_decision: None,
            request_meta: None,
            response_meta: None,
        };

        let mut backend_override = None;
        let mut ctx = PreRequestContext {
            request_id: &request_id,
            request,
            active_backend,
            record: &mut record,
        };

        for plugin in &self.inner.plugins {
            if let Some(override_backend) = plugin.pre_request(&mut ctx) {
                backend_override = Some(override_backend);
            }
        }

        RequestStart {
            span: RequestSpan::new(record),
            backend_override,
        }
    }

    pub fn finish_request(&self, mut span: RequestSpan) {
        span.mark_completed();
        finalize_record(&mut span.record, &span.timing);

        let request_id = span.record.id.clone();
        let mut ctx = PostResponseContext {
            request_id: &request_id,
            record: &mut span.record,
        };
        for plugin in &self.inner.plugins {
            plugin.post_response(&mut ctx);
        }

        self.update_aggregates(&span.record);
        self.inner.ring.push(span.record);
    }

    pub fn finish_error(&self, mut span: RequestSpan, status: Option<u16>) {
        span.record.status = status.or(span.record.status);
        self.finish_request(span);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let recent = self.inner.ring.snapshot();
        let mut per_backend = HashMap::new();

        let aggregates = self.inner.aggregates.read().clone();

        for (backend, acc) in aggregates {
            let mut metrics = BackendMetrics::default();
            metrics.total = acc.total;
            metrics.success_2xx = acc.success_2xx;
            metrics.client_error_4xx = acc.client_error_4xx;
            metrics.server_error_5xx = acc.server_error_5xx;
            metrics.timeouts = acc.timeouts;
            metrics.avg_latency_ms = acc.avg_latency_ms();
            metrics.avg_ttfb_ms = acc.avg_ttfb_ms();
            per_backend.insert(backend, metrics);
        }

        apply_percentiles(&mut per_backend, &recent);

        MetricsSnapshot {
            generated_at: SystemTime::now(),
            per_backend,
            recent,
        }
    }

    fn update_aggregates(&self, record: &RequestRecord) {
        let mut aggregates = self.inner.aggregates.write();

        let entry = aggregates
            .entry(record.backend.clone())
            .or_insert_with(BackendAccumulator::default);
        entry.update(record);
    }
}
