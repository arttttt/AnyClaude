# Observability Infrastructure — Design Document

## Overview

This document defines the observability infrastructure for the proxy server: request/response capture, timing metrics (latency + TTFB), ring-buffer storage, plugin hooks, and aggregated metrics exposure via IPC.

The system is designed to be low-overhead (<0.5ms/request), thread-safe, and extensible without parsing request/response bodies by default.

## Requirements

- Generate a unique request ID for every request.
- Capture start time, first-byte time, end time.
- Compute latency (ms) and TTFB (ms).
- Store last N records in a thread-safe ring buffer (default 1000).
- Provide percentiles (P50/P95/P99) per backend.
- Provide aggregated counts per backend (2xx/4xx/5xx/timeouts).
- Provide extension points for plugins (pre/post hooks) that can enrich records or influence routing.
- Expose metrics via IPC for the dashboard.
- Do not parse body contents by default.

## Current State

- Proxy routing lives in `src/proxy/` and uses `reqwest` for upstream.
- Request IDs are generated in `src/proxy/router.rs` for logging.
- `src/metrics/mod.rs` is a placeholder.
- IPC layer is a placeholder (`src/ipc/mod.rs`).

## Design Decisions

### Decision 1: Observability owned by the proxy

**Choice**: Implement observability in the proxy layer (`src/proxy`) with a shared `ObservabilityHub` that owns storage and plugin registry.

**Rationale**:
- Proxy is the single place that sees full request lifecycle.
- Allows accurate timing (start, first byte, end).
- Avoids coupling UI/runtime to request processing.

### Decision 2: Use monotonic time for durations

**Choice**: Use `Instant` for internal timing and store `SystemTime` for external timestamps.

**Rationale**:
- `Instant` is monotonic and safe for duration calculations.
- `SystemTime` is useful for logs and UI display.

### Decision 3: Minimal body inspection

**Choice**: Compute sizes from byte counts already available in upstream handling (body collection or streaming wrapper). Do not parse JSON or content.

**Rationale**:
- Matches requirement to avoid parsing.
- Keeps overhead low.

### Decision 4: Ring buffer + on-demand percentiles

**Choice**: Store last N records in a `VecDeque` with `RwLock`, compute percentiles on demand using snapshots.

**Rationale**:
- Simple, predictable, and fast for N=1000.
- Avoids complex streaming percentile algorithms initially.

## Architecture

```
┌───────────────────────────┐
│        RouterEngine       │
│  - request_id             │
│  - pre_request hook        │
└────────────┬──────────────┘
             │
             ▼
┌───────────────────────────┐
│       UpstreamClient       │
│  - start/ttfb/end timing   │
│  - response size tracking  │
└────────────┬──────────────┘
             │
             ▼
┌───────────────────────────┐
│     ObservabilityHub       │
│  - RingBuffer<RequestRecord>
│  - Aggregates (per backend)
│  - Plugin registry         │
└────────────┬──────────────┘
             │
             ▼
┌───────────────────────────┐
│            IPC            │
│  - metrics snapshot        │
└───────────────────────────┘
```

## Data Structures

### RequestRecord

```rust
pub struct RequestRecord {
    pub id: String,
    pub started_at: SystemTime,
    pub first_byte_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
    pub latency_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub backend: String,
    pub status: Option<u16>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    // Extensions
    pub request_analysis: Option<RequestAnalysis>,
    pub response_analysis: Option<ResponseAnalysis>,
    pub routing_decision: Option<RoutingDecision>,
}
```

`RequestAnalysis`, `ResponseAnalysis`, and `RoutingDecision` are owned by their respective modules. Initially they can be simple structs or `serde_json::Value` placeholders to avoid tight coupling.

### Ring Buffer

```rust
pub struct RequestRingBuffer {
    capacity: usize,
    records: RwLock<VecDeque<RequestRecord>>,
}

impl RequestRingBuffer {
    pub fn push(&self, record: RequestRecord);
    pub fn snapshot(&self) -> Vec<RequestRecord>;
}
```

## Hooks & Plugin Interface

### Pre-Request Hook

- Runs before the upstream request is sent.
- Creates an initial `RequestRecord` with ID, start timestamp, and request size.
- Allows plugins to enrich record and propose a backend override.

```rust
pub struct PreRequestContext<'a> {
    pub request_id: &'a str,
    pub request: &'a Request<Body>,
    pub backend: &'a str,
    pub record: &'a mut RequestRecord,
}

pub enum BackendDecision {
    UseActive,
    Override(String),
}

pub trait ObservabilityPlugin: Send + Sync {
    fn pre_request(&self, ctx: &mut PreRequestContext) -> BackendDecision {
        BackendDecision::UseActive
    }

    fn post_response(&self, ctx: &mut PostResponseContext) {}
}
```

### Post-Response Hook

- Runs after response completes (including streaming end).
- Finalizes timestamps, sizes, status code.
- Allows plugins to enrich record and compute derived data.

```rust
pub struct PostResponseContext<'a> {
    pub request_id: &'a str,
    pub response_status: Option<u16>,
    pub record: &'a mut RequestRecord,
}
```

## Timing Strategy

- `started_at`: set in pre-request hook (`SystemTime::now()`), plus `Instant::now()` stored internally in a `RequestTiming` helper.
- `first_byte_at`: set when the first response chunk is observed.
- `completed_at`: set after full response body is sent to client (or streaming ends).
- `latency_ms`: `(completed_at - started_at)` computed using `Instant`.
- `ttfb_ms`: `(first_byte_at - started_at)` computed using `Instant`.

Implementation detail: for streaming responses, wrap the response body in a `Stream` adapter that records first chunk time and counts bytes until the stream ends.

## Aggregated Metrics

### Per-backend Counters

Maintain counters updated on record insert:

- total requests
- 2xx count
- 4xx/5xx count
- timeouts
- average latency
- average TTFB

### Percentiles

On-demand calculation from ring buffer snapshot:

- `P50`, `P95`, `P99` for latency per backend
- Optional overall percentiles (across all backends)

## IPC Exposure

Expose a snapshot via IPC for TUI dashboard:

```rust
pub struct MetricsSnapshot {
    pub generated_at: SystemTime,
    pub per_backend: HashMap<String, BackendMetrics>,
    pub recent: Vec<RequestRecord>,
}
```

The IPC layer will request a snapshot from `ObservabilityHub` and serialize it to JSON.

## Implementation Plan

1. Add `src/metrics/observability.rs` (or expand `src/metrics/mod.rs`) with:
   - `RequestRecord`
   - `RequestRingBuffer`
   - `ObservabilityHub`
   - `ObservabilityPlugin` trait
2. Integrate hooks into `RouterEngine` and `UpstreamClient`.
3. Add response body wrapper for streaming TTFB + size tracking.
4. Add aggregation helpers and snapshot method.
5. Add IPC endpoint to retrieve metrics snapshot.

## Acceptance Criteria Mapping

- Unique request ID: generated in router, stored in `RequestRecord`.
- Timing accuracy: `Instant` + body wrapper for first-byte and completion.
- Ring buffer correctness: `VecDeque` with capacity.
- Percentiles: computed from snapshot using sorted latencies.
- Plugins: trait with pre/post hooks.
- IPC: snapshot struct exposed via IPC layer.
- Overhead: bounded by O(1) inserts and small locks.

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Streaming completion not detected | Use body wrapper that finalizes on stream end/drop |
| Lock contention under load | Keep records small, write once per request |
| Percentiles expensive | Snapshot size capped (N=1000) |
| Plugin failures | Catch panics and log, do not fail proxy |

## Open Questions

1. Should request ID be returned as `x-request-id` header?
2. Should ring buffer store errors when request fails before response?
3. Should percentiles be computed incrementally for better perf?
