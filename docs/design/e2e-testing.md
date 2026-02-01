# End-to-End Testing Design

## Overview

Comprehensive testing suite covering all critical user workflows and system integration points.

## Current Test Coverage

### Existing Tests (40 tests, all passing)

| Test File | Tests | Coverage |
|-----------|-------|----------|
| `config_loader.rs` | 15 | Config parsing, validation, auth headers |
| `request_parser_test.rs` | 9 | Request body parsing, model/tools/images detection |
| `test_shutdown.rs` | 8 | ShutdownCoordinator, ShutdownManager |
| `pty_passthrough.rs` | 3+1 | PTY spawn, input echo, resize |
| `integration_health.rs` | 2 | Health endpoint, request forwarding |
| `test_sse_streaming.rs` | 2 | SSE content-type detection |
| `cli_args.rs` | 4 | CLI argument parsing |
| `provider_thinking_compat.rs` | 8 (ignored) | Cross-provider thinking blocks |

### Inline Unit Tests

- `src/backend/state.rs` - BackendState switching, validation
- `src/ipc/tests.rs` - IPC layer communication
- `src/config/watcher.rs` - ConfigStore reload
- `src/proxy/thinking.rs` - Thinking block transformation
- `src/proxy/pool.rs` - Pool configuration
- `src/proxy/timeout.rs` - Timeout configuration

## Testing Gaps

### 1. Proxy Integration (High Priority)
- Backend switching during active requests
- Request retry with exponential backoff
- Connection pooling behavior
- Auth header injection/stripping

### 2. Mock Backend Server (Critical Infrastructure)
Current tests hit real endpoints or skip. Need:
- Configurable response delays
- SSE streaming simulation
- Error response simulation
- Request capture for assertions

### 3. Configuration Hot-Reload
- Config file changes trigger reload
- Invalid config preserves old state
- Backend removed during active use

### 4. Error Recovery
- Network failures mid-stream
- Timeout scenarios
- Malformed responses

## Test Infrastructure Design

### Mock Backend Server

```rust
// tests/common/mock_backend.rs
pub struct MockBackend {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    responses: Arc<Mutex<VecDeque<MockResponse>>>,
}

impl MockBackend {
    pub async fn start() -> Self;
    pub fn enqueue_response(&self, resp: MockResponse);
    pub fn enqueue_sse_stream(&self, events: Vec<&str>);
    pub fn captured_requests(&self) -> Vec<CapturedRequest>;
}
```

### Test Helpers

```rust
// tests/common/mod.rs
pub fn temp_config(backends: &[(&str, &str)]) -> (TempDir, PathBuf);
pub fn free_port() -> u16;
pub async fn wait_for_server(addr: SocketAddr, timeout: Duration);
```

## New Test Modules

### 1. `tests/proxy_backend_switch.rs`

```rust
#[tokio::test]
async fn test_backend_switch_preserves_inflight_request();

#[tokio::test]
async fn test_concurrent_requests_different_backends();

#[tokio::test]
async fn test_switch_to_unconfigured_backend_fails();
```

### 2. `tests/proxy_retry.rs`

```rust
#[tokio::test]
async fn test_retry_on_connection_error();

#[tokio::test]
async fn test_exponential_backoff();

#[tokio::test]
async fn test_max_retries_exceeded();
```

### 3. `tests/proxy_streaming.rs`

```rust
#[tokio::test]
async fn test_sse_passthrough();

#[tokio::test]
async fn test_streaming_timeout();

#[tokio::test]
async fn test_stream_interruption_cleanup();
```

### 4. `tests/config_hotreload.rs`

```rust
#[tokio::test]
async fn test_config_reload_updates_backends();

#[tokio::test]
async fn test_invalid_config_preserves_old();

#[tokio::test]
async fn test_active_backend_removed();
```

### 5. `tests/e2e_workflow.rs`

```rust
#[tokio::test]
async fn test_full_lifecycle_start_request_shutdown();

#[tokio::test]
async fn test_graceful_shutdown_drains_connections();
```

## Implementation Plan

1. **Create test infrastructure** (`tests/common/`)
   - MockBackend server with axum
   - Port allocation helper
   - Temp config generator

2. **Add proxy integration tests**
   - Backend switching
   - Retry logic
   - Auth header handling

3. **Add streaming tests**
   - SSE passthrough
   - Timeout handling

4. **Add config reload tests**
   - Hot reload with file watcher
   - Error handling

5. **Add e2e workflow tests**
   - Full lifecycle
   - Graceful shutdown

## Dependencies

Add to `Cargo.toml` dev-dependencies:
```toml
[dev-dependencies]
tempfile = "3.24"  # already present
tokio-test = "0.4"
```

## Acceptance Criteria

- [ ] MockBackend server implemented
- [ ] Test helpers in `tests/common/`
- [ ] Backend switching tests pass
- [ ] Retry logic tests pass
- [ ] Streaming tests pass
- [ ] Config reload tests pass
- [ ] E2E workflow tests pass
- [ ] All 40+ existing tests still pass
