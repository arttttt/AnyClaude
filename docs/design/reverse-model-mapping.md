# Reverse Model Mapping in Proxy Responses

## Status: IMPLEMENTED

## Problem

The proxy rewrites model names in requests (forward mapping):

```
CC sends: {"model": "claude-opus-4-6"}
Proxy rewrites: {"model": "glm-5"}
Backend receives: {"model": "glm-5"}
```

But the response was passed through AS-IS:

```
Backend returns SSE: data: {"type":"message_start","message":{"model":"glm-5",...}}
CC receives: model = "glm-5"  (unexpected)
```

CC expects to see the same model name it sent. Receiving an alien model name can cause:
- Model mismatch warnings in CC logs
- Incorrect model attribution in conversation metadata
- Potential feature gating issues (CC may enable/disable features based on model name)

## Architecture

### Forward Mapping (Request)

`upstream.rs` — rewrites `model` field in JSON body before sending upstream using
`backend.resolve_model()` (keyword-based family matching: opus/sonnet/haiku).

### Reverse Mapping (Response) — NEW

Implemented in `src/proxy/model_rewrite.rs`. Uses `ModelMapping` struct to carry the
backend/original model name pair with semantic field names.

Two paths:

**Streaming (SSE):** A stateful `ChunkRewriter` closure is attached to `ObservedStream`.
It performs a byte-level pre-check (`contains_bytes(haystack, b"\"message_start\"")`) to
skip chunks without `message_start` without any parsing overhead. When `message_start` is
found, it parses SSE lines, locates the `data:` line with the `message_start` JSON, replaces
`message.model` with the original model name, and reconstructs the SSE chunk. After
processing, the rewriter becomes a zero-cost no-op for all subsequent chunks.

Note: The rewriter intentionally re-implements SSE line parsing rather than reusing
`sse::parse_sse_events()`. That function discards non-data lines (event:, empty) and line
structure, making it impossible to reconstruct the original SSE text with modifications.
The rewriter needs in-place transformation with full line reconstruction. See cross-reference
comment in `model_rewrite.rs:57-61`.

**Non-streaming (JSON):** The full response body is parsed as JSON, `$.model` is replaced,
and the body is re-serialized.

Both paths log a warning when the backend returns a model that doesn't match the expected
backend model name, aiding debugging of misconfigurations.

### ChunkRewriter Lifecycle

```text
[Waiting] ──chunk without message_start──> [Waiting]  (pass through, no parsing)
[Waiting] ──chunk with message_start─────> [Done]     (rewrite model, mark done)
[Done]    ──any chunk────────────────────> [Done]      (zero-cost pass through)
```

### Content-Length Handling

When reverse mapping is active, the `Content-Length` header from the upstream response is
stripped. This is necessary because the body size changes after model name substitution
(e.g., "glm-5" → "claude-opus-4-6" is longer), making the original value stale.

For **streaming responses**, the absence of Content-Length is standard (SSE uses chunked
transfer encoding by nature).

For **non-streaming responses**, stripping the upstream Content-Length causes hyper to
recalculate it from the actual body size, so the client receives a correct Content-Length
matching the rewritten body.

### SSE Chunk Boundary Edge Case

`message_start` is always the first SSE event and is typically <500 bytes. Testing with
real traffic confirmed it always arrives complete in chunk #0 (278-314 bytes observed).

If a split were to occur (extremely rare for small events), the byte-level pre-check
would fail to find `"message_start"` in either half-chunk. The rewriter does NOT mark
itself as done on a miss — it remains in `[Waiting]` state and continues checking
subsequent chunks. This means a split `message_start` would pass through unchanged
(identical to pre-implementation behavior), and the rewriter would keep looking until
the stream ends. No regression risk.

## Files Changed

| File | Change |
|------|--------|
| `src/proxy/model_rewrite.rs` | `ModelMapping` struct, `make_reverse_model_rewriter()`, `reverse_model_in_response()` |
| `src/proxy/upstream.rs` | `model_mapping` capture, Content-Length stripping, calls to `model_rewrite` module |
| `src/proxy/mod.rs` | `pub mod model_rewrite` |
| `src/metrics/stream.rs` | `ChunkRewriter` type alias (`Send` only, no `Sync`), `chunk_rewriter` field, `with_chunk_rewriter()` builder |
| `src/metrics/mod.rs` | Re-export `ChunkRewriter` |
| `tests/reverse_model_mapping.rs` | 24 unit tests + 8 integration tests (32 total) |
| `tests/common/mock_backend.rs` | `MockResponse::sse()` now generates realistic `event:` + `data:` lines |

## Testing

### Unit Tests (24)

- **SSE rewriter (16):** standard rewrite, skip non-message-start, model mismatch (with
  log), stateful no-op after first rewrite, second message_start ignored, empty chunk,
  ping event, mixed events (ping + message_start), compact `data:` format, model with
  version suffix, field preservation, missing message object, missing model field,
  non-UTF8 bytes, unicode model name, malformed SSE data line
- **Non-streaming JSON (8):** standard rewrite, model mismatch, invalid JSON, no model
  field, empty body, field preservation, error response JSON, binary body

### Integration Tests (8)

Full proxy pipeline tests using `MockBackend`:
- SSE streaming with opus model mapping → verifies reverse mapping + Content-Length stripped
- Non-streaming JSON with opus mapping → verifies model rewritten + Content-Length correct
- No mapping configured (passthrough) → verifies no changes
- SSE with model mismatch (unexpected model) → verifies passthrough
- SSE with sonnet model family → verifies sonnet reverse mapping
- JSON with haiku model family → verifies haiku reverse mapping
- Concurrent requests with independent rewriters → verifies isolation
- Error response (4xx) with model field → verifies rewriting in error responses

Integration tests also verify:
- Forward mapping via `mock.captured_requests()` (C2)
- Content-Length header correctness/stripping (C1)

## Performance Impact

| Operation | Cost | Frequency |
|-----------|------|-----------|
| Byte-level `contains_bytes` check | O(n), 0 alloc | Every chunk until `message_start` found (~1 chunk) |
| SSE line parse + JSON rewrite | O(n), 1 alloc | Once per response (only the message_start chunk) |
| JSON parse + serialize (non-streaming) | O(n), small alloc | Once per non-streaming response |
| Skip (fast path, `done=true`) | Zero-cost | All chunks after first (~dozens per response) |

The rewriter is stateful: after processing the first chunk (which contains `message_start`),
it sets `done=true` and returns all subsequent chunks unchanged with zero overhead.
