# Debug Mode & Request Logging - Design Document

## Overview

This document defines a Debug Logger plugin for the Observability Infrastructure. The logger provides request-level diagnostics with configurable verbosity, output formats, and destinations, while keeping the default overhead at zero.

## Goals

- Provide detailed request/response logs for debugging production issues.
- Support multiple log levels (off/basic/verbose/full).
- Support console and JSONL output to stderr, file, or both.
- Ensure secrets never appear in logs.
- Allow runtime enable/disable via IPC without restart.

## Non-Goals

- Replace tracing-based internal logs.
- Persist full request/response bodies.
- Provide long-term metrics storage (handled by ObservabilityHub ring buffer).

## Requirements

- Off mode has zero additional overhead beyond an atomic check.
- Basic mode logs timestamp, method, path, backend, latency, status.
- Verbose mode adds model, token counts, images, routing decision, cost, stop reason.
- Full mode adds headers (redacted) and request/response body previews.
- Output formats: human-readable console or JSONL.
- Destinations: stderr, file, or both, with rotation.
- Enable/disable by env var, config file, and runtime IPC.

## Current State

- ObservabilityHub supports plugins with pre_request/post_response hooks.
- RequestRecord has timing, status, bytes, routing decision, and analysis fields.
- UpstreamClient already reads request bodies and non-streaming responses.
- Streaming responses are wrapped by ObservedStream for timing/bytes only.
- Tracing supports optional file logging via CLAUDE_WRAPPER_LOG (unrelated).

## Design Decisions

### Decision 1: Implement DebugLogger as ObservabilityPlugin

**Approach:** Add a DebugLogger plugin that runs in post_response, using RequestRecord plus additional optional debug fields.

**Rationale:** Keeps logging tied to the full request lifecycle and uses existing plugin infrastructure.

### Decision 2: Runtime Configuration with Atomic Snapshot

**Approach:** DebugLogger holds an Arc<DebugConfig> wrapped in Atomic/ArcSwap. IPC updates swap the config. The plugin checks level first and returns immediately when off.

**Rationale:** Enables runtime toggling without restart and avoids overhead when disabled.

### Decision 3: Extend RequestRecord with Optional Debug Fields

**Approach:** Add optional debug metadata to RequestRecord for logging, populated only when level >= verbose/full.

Proposed additions:

```rust
pub struct RequestRecord {
    // existing fields...
    pub request_meta: Option<RequestMeta>,
    pub response_meta: Option<ResponseMeta>,
}

pub struct RequestMeta {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: Option<Vec<(String, String)>>,
    pub body_preview: Option<String>,
}

pub struct ResponseMeta {
    pub headers: Option<Vec<(String, String)>>,
    pub body_preview: Option<String>,
    pub output_tokens: Option<u64>,
    pub stop_reason: Option<String>,
    pub cost_usd: Option<f64>,
}
```

**Rationale:** Keeps core metrics light while allowing richer logs in debug mode.

### Decision 4: Capture Request/Response Previews at Proxy Boundary

**Approach:**
- Request previews are captured in UpstreamClient when request bodies are already collected.
- Response previews are captured for non-streaming responses from the bytes buffer.
- For streaming responses, wrap ObservedStream with a tee that captures the first N bytes and final event summary (best effort).

**Rationale:** Avoids additional body reads and limits memory usage.

### Decision 5: Parsing for Verbose Fields

**Approach:**
- Use RequestParser to populate RequestRecord.request_analysis in UpstreamClient after body collection.
- Add ResponseParser to parse non-streaming JSON (and SSE final event where possible) to extract output tokens, stop reason, and usage fields.
- Cost is computed from backend pricing if available (optional, config-driven).

**Rationale:** Meets verbose requirements without logging full bodies.

### Decision 6: Redaction Strategy

**Approach:**
- Always redact sensitive headers: Authorization, Proxy-Authorization, X-Api-Key, Cookie, Set-Cookie.
- If JSON, redact keys: api_key, authorization, access_token, refresh_token, secret, password.
- For non-JSON previews, apply token masking with a conservative regex (prefix + last 4 chars only).

**Rationale:** Ensures secrets never leak while preserving diagnostic value.

### Decision 7: Output Formats and Destinations

**Console:** compact one-line summary, with optional multi-line blocks for full mode.

**JSONL:** one JSON object per line, includes full DebugLogEvent with redacted data.

**Destinations:** stderr, file, or both. File output uses rotation.

### Decision 8: File Rotation

**Approach:**
- Size-based rotation: rotate at max_bytes, keep max_files.
- Time-based rotation: rotate daily with suffix.
- Implement via a small writer utility (or external crate if acceptable).

**Rationale:** Prevents unbounded log growth.

## Configuration

### Config File

```toml
[debug_logging]
level = "off"          # off | basic | verbose | full
format = "console"     # console | json
destination = "stderr" # stderr | file | both
file_path = "~/.config/anyclaude/debug.log"
body_preview_bytes = 1024
header_preview = true

[debug_logging.rotation]
mode = "size"          # size | daily
max_bytes = 10485760    # 10 MB
max_files = 5
```

### Environment Variables

- CLAUDE_WRAPPER_DEBUG_LEVEL
- CLAUDE_WRAPPER_DEBUG_FORMAT
- CLAUDE_WRAPPER_DEBUG_DEST
- CLAUDE_WRAPPER_DEBUG_FILE
- CLAUDE_WRAPPER_DEBUG_BODY_PREVIEW_BYTES

### Activation Precedence

1. IPC runtime override
2. Environment variables
3. Config file
4. Default: off

## IPC Interface

Add IPC commands:

```rust
pub enum IpcCommand {
    SetDebugLogging {
        config: DebugLoggingConfig,
        respond_to: oneshot::Sender<Result<(), IpcError>>,
    },
    GetDebugLogging {
        respond_to: oneshot::Sender<DebugLoggingConfig>,
    },
}
```

DebugLoggingConfig is shared between config and IPC and stored in DebugLogger.

## Logging Output

### Basic (Console)

```
2026-02-02T00:13:08Z POST /v1/messages backend=anthropic status=200 latency_ms=842
```

### Verbose (Console)

```
2026-02-02T00:13:08Z POST /v1/messages backend=anthropic status=200 latency_ms=842
model=claude-3-5-sonnet input_tokens=732 output_tokens=412 images=0 stop_reason=end_turn
routing=rule:thinking_compat cost_usd=0.0087
```

### Full (JSONL)

```json
{"ts":"2026-02-02T00:13:08Z","level":"full","method":"POST","path":"/v1/messages","backend":"anthropic","status":200,"latency_ms":842,"request":{"headers":[["content-type","application/json"],["authorization","Bearer ****abcd"]],"body_preview":"{\"model\":\"..."},"response":{"headers":[["content-type","application/json"]],"body_preview":"{\"id\":\"..."}}}
```

## Implementation Plan

1. Add DebugLoggingConfig to config types with defaults and env override parsing.
2. Extend RequestRecord with RequestMeta/ResponseMeta (optional).
3. Capture method/path/query in RouterEngine and store in span record.
4. Capture request body preview in UpstreamClient when reading request body.
5. Capture response body preview for non-streaming responses; add a tee for streaming.
6. Add ResponseParser for token counts/stop reason (best effort).
7. Implement DebugLogger plugin with async writer and rotation.
8. Add IPC commands to get/set debug logging config.

## Testing Strategy

- Unit tests for redaction (headers and JSON keys).
- Unit tests for log formatting (console + JSON).
- Integration tests for IPC toggling.
- Verify Off mode adds no extra allocations (bench or lightweight perf test).

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| TUI corruption when logging to stderr | Default off; document that debug logging may affect TUI; recommend file output | 
| Performance regressions in Full mode | Cap preview sizes and only capture when enabled | 
| Streaming parsing complexity | Best-effort parsing; if unavailable, omit tokens/stop_reason | 
| Secret leakage | Strict redaction with allowlist + test coverage |

## Open Questions

1. Should debug logger reuse tracing output infrastructure or stay separate?
2. Do we want to include request_id as an x-request-id response header?
3. Should preview bytes be counted toward response_bytes, or separate?
