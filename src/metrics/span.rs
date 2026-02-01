use std::time::{Duration, Instant, SystemTime};

use super::types::{BackendOverride, RequestRecord};

pub struct RequestStart {
    pub span: RequestSpan,
    pub backend_override: Option<BackendOverride>,
}

pub struct RequestSpan {
    pub(crate) record: RequestRecord,
    pub(crate) timing: RequestTiming,
}

impl RequestSpan {
    pub fn new(record: RequestRecord) -> Self {
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

pub struct RequestTiming {
    pub(crate) started_instant: Instant,
    pub(crate) first_byte_instant: Option<Instant>,
    pub(crate) completed_instant: Option<Instant>,
}

impl RequestTiming {
    pub fn new() -> Self {
        Self {
            started_instant: Instant::now(),
            first_byte_instant: None,
            completed_instant: None,
        }
    }

    pub fn mark_first_byte(&mut self) {
        if self.first_byte_instant.is_none() {
            self.first_byte_instant = Some(Instant::now());
        }
    }

    pub fn mark_completed(&mut self) {
        if self.completed_instant.is_none() {
            self.completed_instant = Some(Instant::now());
        }
    }
}

pub fn finalize_record(record: &mut RequestRecord, timing: &RequestTiming) {
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
