pub mod request_parser;

use axum::body::Body;
use axum::http::Request;
use futures_core::Stream;
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime};

use axum::body::Bytes;

pub use request_parser::{RequestAnalysis, RequestParser};

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
    pub request_analysis: Option<RequestAnalysis>,
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

pub trait ObservabilityPlugin: Send + Sync {
    fn pre_request(&self, _ctx: &mut PreRequestContext<'_>) -> Option<BackendOverride> {
        None
    }

    fn post_response(&self, _ctx: &mut PostResponseContext<'_>) {}
}

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
        };

        let mut backend_override: Option<BackendOverride> = None;
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

        let aggregates = self
            .inner
            .aggregates
            .read()
            .expect("metrics aggregates lock poisoned")
            .clone();

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
        let mut aggregates = self
            .inner
            .aggregates
            .write()
            .expect("metrics aggregates lock poisoned");

        let entry = aggregates
            .entry(record.backend.clone())
            .or_insert_with(BackendAccumulator::default);
        entry.update(record);
    }
}

#[derive(Clone)]
pub struct RequestRingBuffer {
    capacity: usize,
    records: Arc<RwLock<VecDeque<RequestRecord>>>,
}

impl RequestRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            records: Arc::new(RwLock::new(VecDeque::with_capacity(capacity))),
        }
    }

    pub fn push(&self, record: RequestRecord) {
        let mut records = self.records.write().expect("ring buffer lock poisoned");
        if records.len() == self.capacity {
            records.pop_front();
        }
        records.push_back(record);
    }

    pub fn snapshot(&self) -> Vec<RequestRecord> {
        let records = self.records.read().expect("ring buffer lock poisoned");
        records.iter().cloned().collect()
    }
}

pub struct RequestStart {
    pub span: RequestSpan,
    pub backend_override: Option<BackendOverride>,
}

pub struct RequestSpan {
    record: RequestRecord,
    timing: RequestTiming,
}

impl RequestSpan {
    fn new(record: RequestRecord) -> Self {
        Self {
            record,
            timing: RequestTiming::new(),
        }
    }

    pub fn set_backend(&mut self, backend: String) {
        self.record.backend = backend;
    }

    pub fn set_status(&mut self, status: u16) {
        self.record.status = Some(status);
    }

    pub fn set_request_bytes(&mut self, bytes: usize) {
        self.record.request_bytes = bytes as u64;
    }

    pub fn add_response_bytes(&mut self, bytes: usize) {
        self.record.response_bytes = self.record.response_bytes.saturating_add(bytes as u64);
    }

    pub fn mark_first_byte(&mut self) {
        self.timing.mark_first_byte();
    }

    pub fn mark_completed(&mut self) {
        self.timing.mark_completed();
    }

    pub fn mark_timed_out(&mut self) {
        self.record.timed_out = true;
    }

    pub fn record_mut(&mut self) -> &mut RequestRecord {
        &mut self.record
    }
}

struct RequestTiming {
    started_instant: Instant,
    first_byte_instant: Option<Instant>,
    completed_instant: Option<Instant>,
}

impl RequestTiming {
    fn new() -> Self {
        Self {
            started_instant: Instant::now(),
            first_byte_instant: None,
            completed_instant: None,
        }
    }

    fn mark_first_byte(&mut self) {
        if self.first_byte_instant.is_none() {
            self.first_byte_instant = Some(Instant::now());
        }
    }

    fn mark_completed(&mut self) {
        if self.completed_instant.is_none() {
            self.completed_instant = Some(Instant::now());
        }
    }
}

pub struct ObservedStream<S> {
    inner: S,
    span: Option<RequestSpan>,
    hub: ObservabilityHub,
}

impl<S> ObservedStream<S> {
    pub fn new(inner: S, span: RequestSpan, hub: ObservabilityHub) -> Self {
        Self {
            inner,
            span: Some(span),
            hub,
        }
    }

    fn finish(&mut self) {
        if let Some(span) = self.span.take() {
            self.hub.finish_request(span);
        }
    }
}

impl<S> Stream for ObservedStream<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<Bytes, reqwest::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                if let Some(span) = &mut self.span {
                    span.mark_first_byte();
                    span.add_response_bytes(bytes.len());
                }
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(err))) => {
                self.finish();
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                self.finish();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> Drop for ObservedStream<S> {
    fn drop(&mut self) {
        self.finish();
    }
}

#[derive(Default, Clone)]
struct BackendAccumulator {
    total: u64,
    success_2xx: u64,
    client_error_4xx: u64,
    server_error_5xx: u64,
    timeouts: u64,
    latency_total_ms: u64,
    latency_samples: u64,
    ttfb_total_ms: u64,
    ttfb_samples: u64,
}

impl BackendAccumulator {
    fn update(&mut self, record: &RequestRecord) {
        self.total += 1;
        if let Some(status) = record.status {
            if (200..300).contains(&status) {
                self.success_2xx += 1;
            } else if (400..500).contains(&status) {
                self.client_error_4xx += 1;
            } else if (500..600).contains(&status) {
                self.server_error_5xx += 1;
            }
        }

        // Track timeouts from both 504 status and reqwest timeout errors
        if record.timed_out || record.status == Some(504) {
            self.timeouts += 1;
        }

        if let Some(latency_ms) = record.latency_ms {
            self.latency_total_ms = self.latency_total_ms.saturating_add(latency_ms);
            self.latency_samples += 1;
        }

        if let Some(ttfb_ms) = record.ttfb_ms {
            self.ttfb_total_ms = self.ttfb_total_ms.saturating_add(ttfb_ms);
            self.ttfb_samples += 1;
        }
    }

    fn avg_latency_ms(&self) -> f64 {
        if self.latency_samples == 0 {
            return 0.0;
        }
        self.latency_total_ms as f64 / self.latency_samples as f64
    }

    fn avg_ttfb_ms(&self) -> f64 {
        if self.ttfb_samples == 0 {
            return 0.0;
        }
        self.ttfb_total_ms as f64 / self.ttfb_samples as f64
    }
}

fn finalize_record(record: &mut RequestRecord, timing: &RequestTiming) {
    if record.completed_at.is_none() {
        record.completed_at = Some(SystemTime::now());
    }

    if let Some(first_byte) = timing.first_byte_instant {
        let ttfb = first_byte.duration_since(timing.started_instant);
        record.ttfb_ms = Some(ttfb.as_millis() as u64);
        record.first_byte_at = record.started_at.checked_add(ttfb);
    }

    if let Some(completed) = timing.completed_instant {
        let latency = completed.duration_since(timing.started_instant);
        record.latency_ms = Some(latency.as_millis() as u64);
        record.completed_at = record
            .started_at
            .checked_add(latency)
            .or(record.completed_at);
    }

    if record.first_byte_at.is_none() && record.ttfb_ms.is_some() {
        let ttfb = Duration::from_millis(record.ttfb_ms.unwrap_or(0));
        record.first_byte_at = record.started_at.checked_add(ttfb);
    }
}

fn apply_percentiles(per_backend: &mut HashMap<String, BackendMetrics>, records: &[RequestRecord]) {
    let mut per_backend_latencies: HashMap<String, Vec<u64>> = HashMap::new();

    for record in records {
        if let (Some(latency), backend) = (record.latency_ms, &record.backend) {
            per_backend_latencies
                .entry(backend.clone())
                .or_default()
                .push(latency);
        }
    }

    for (backend, mut values) in per_backend_latencies {
        values.sort_unstable();
        let metrics = per_backend.entry(backend).or_default();
        metrics.p50_latency_ms = percentile(&values, 0.50);
        metrics.p95_latency_ms = percentile(&values, 0.95);
        metrics.p99_latency_ms = percentile(&values, 0.99);
    }
}

fn percentile(values: &[u64], percentile: f64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let rank = (values.len().saturating_sub(1) as f64 * percentile).round() as usize;
    values.get(rank).copied()
}
