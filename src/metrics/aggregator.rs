use std::collections::HashMap;

use super::types::{BackendMetrics, RequestRecord};

#[derive(Default, Clone)]
pub struct BackendAccumulator {
    pub(crate) total: u64,
    pub(crate) success_2xx: u64,
    pub(crate) client_error_4xx: u64,
    pub(crate) server_error_5xx: u64,
    pub(crate) timeouts: u64,
    latency_total_ms: u64,
    latency_samples: u64,
    ttfb_total_ms: u64,
    ttfb_samples: u64,
}

impl BackendAccumulator {
    pub fn update(&mut self, record: &RequestRecord) {
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

    pub fn avg_latency_ms(&self) -> f64 {
        if self.latency_samples == 0 {
            return 0.0;
        }
        self.latency_total_ms as f64 / self.latency_samples as f64
    }

    pub fn avg_ttfb_ms(&self) -> f64 {
        if self.ttfb_samples == 0 {
            return 0.0;
        }
        self.ttfb_total_ms as f64 / self.ttfb_samples as f64
    }
}

pub fn apply_percentiles(
    per_backend: &mut HashMap<String, BackendMetrics>,
    records: &[RequestRecord],
) {
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
