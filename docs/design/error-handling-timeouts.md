# Error Handling and Timeouts — Design Document

## Overview

This document defines the error handling strategy and timeout configuration for the AnyClaude proxy server. The design ensures robust request handling, clear error propagation, and configurable timeout behavior.

## Current State Analysis

### Existing Code

1. **Config Types** (`src/config/types.rs`):
   - `Defaults.timeout_seconds: u32` - Already defined but unused
   - Default value: 30 seconds

2. **Upstream Client** (`src/proxy/upstream.rs`):
   - Uses `anyhow::Result` for error handling
   - No timeout enforcement on HTTP requests
   - Basic 502 Bad Gateway error for connection failures

3. **Error Handling**:
   - Uses `thiserror` for structured config errors (`ConfigError`)
   - Uses `anyhow` for proxy-level errors
   - No structured error types for proxy operations

## Design Decisions

### Decision 1: Structured Error Types

**Approach**: Create a dedicated `ProxyError` enum for all proxy-related errors.

**Rationale**:
- Enables precise error classification and handling
- Supports proper HTTP status code mapping
- Allows detailed error logging and metrics
- Makes error cases explicit and testable

**Error Categories**:
1. **Configuration Errors** - Invalid or missing backend config
2. **Connection Errors** - Cannot reach upstream server
3. **Timeout Errors** - Request exceeded timeout limit
4. **Protocol Errors** - Invalid HTTP/response handling
5. **Auth Errors** - Credential resolution failures

### Decision 2: Timeout Strategy

**Approach**: Implement three distinct timeout layers:

1. **Connection Timeout** (5s default): Time to establish TCP connection
2. **Request Timeout** (from config): Total time for complete request/response
3. **Idle Timeout** (60s default): Time between bytes for streaming responses

**Rationale**:
- Connection timeout prevents hanging on unreachable backends
- Request timeout enforces overall SLA
- Idle timeout handles slow streaming responses without breaking valid streams

### Decision 3: Timeout Configuration

**Approach**: Extend existing config with timeout fields:

```toml
[defaults]
active = "anthropic"
timeout_seconds = 30          # Total request timeout
connect_timeout_seconds = 5   # TCP connection timeout
idle_timeout_seconds = 60     # Streaming idle timeout
```

**Rationale**:
- Uses existing `timeout_seconds` field (already in Config)
- Adds granular control for different timeout scenarios
- Backwards compatible (defaults provided)

### Decision 4: Error Response Format

**Approach**: Standardized JSON error responses:

```json
{
  "error": {
    "type": "timeout",
    "message": "Request exceeded 30s timeout",
    "backend": "anthropic",
    "request_id": "uuid-v4"
  }
}
```

**Rationale**:
- Machine-readable error types for client handling
- Human-readable messages for debugging
- Request IDs for distributed tracing
- Consistent format across all error types

## Implementation Architecture

### Module Structure

```
src/proxy/
├── mod.rs           # Existing: ProxyServer
├── router.rs        # Existing: RouterEngine (updated)
├── upstream.rs      # Existing: UpstreamClient (updated)
├── health.rs        # Existing: HealthHandler
├── shutdown.rs      # Existing: ShutdownManager
├── error.rs         # NEW: ProxyError, ErrorResponse
└── timeout.rs       # NEW: TimeoutConfig, TimeoutLayer
```

### Error Type Hierarchy

```rust
// src/proxy/error.rs

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),
    
    #[error("Backend '{backend}' not found")]
    BackendNotFound { backend: String },
    
    #[error("Backend '{backend}' not configured: {reason}")]
    BackendNotConfigured { backend: String, reason: String },
    
    #[error("Connection failed to '{backend}': {source}")]
    ConnectionError { 
        backend: String, 
        #[source]
        source: hyper_util::client::legacy::Error 
    },
    
    #[error("Request timeout after {duration}s")]
    RequestTimeout { duration: u64 },
    
    #[error("Idle timeout after {duration}s of inactivity")]
    IdleTimeout { duration: u64 },
    
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    
    #[error("Upstream error: {status} - {message}")]
    UpstreamError { status: u16, message: String },
    
    #[error("Internal error: {0}")]
    Internal(String),
}

impl ProxyError {
    /// Map error to HTTP status code
    pub fn status_code(&self) -> StatusCode {
        match self {
            ProxyError::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ProxyError::BackendNotFound { .. } => StatusCode::BAD_GATEWAY,
            ProxyError::BackendNotConfigured { .. } => StatusCode::BAD_GATEWAY,
            ProxyError::ConnectionError { .. } => StatusCode::BAD_GATEWAY,
            ProxyError::RequestTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
            ProxyError::IdleTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
            ProxyError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            ProxyError::UpstreamError { status, .. } => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY)
            }
            ProxyError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
    
    /// Get error type string for JSON response
    pub fn error_type(&self) -> &'static str {
        match self {
            ProxyError::Config(_) => "config_error",
            ProxyError::BackendNotFound { .. } => "backend_not_found",
            ProxyError::BackendNotConfigured { .. } => "backend_not_configured",
            ProxyError::ConnectionError { .. } => "connection_error",
            ProxyError::RequestTimeout { .. } => "request_timeout",
            ProxyError::IdleTimeout { .. } => "idle_timeout",
            ProxyError::InvalidRequest(_) => "invalid_request",
            ProxyError::UpstreamError { .. } => "upstream_error",
            ProxyError::Internal(_) => "internal_error",
        }
    }
}
```

### Timeout Configuration

```rust
// src/proxy/timeout.rs

use std::time::Duration;
use crate::config::types::Defaults;

/// Timeout configuration for proxy requests
#[derive(Debug, Clone, Copy)]
pub struct TimeoutConfig {
    /// Time to establish TCP connection
    pub connect: Duration,
    /// Total time for complete request/response
    pub request: Duration,
    /// Max time between bytes for streaming
    pub idle: Duration,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            request: Duration::from_secs(30),
            idle: Duration::from_secs(60),
        }
    }
}

impl From<&Defaults> for TimeoutConfig {
    fn from(defaults: &Defaults) -> Self {
        Self {
            connect: Duration::from_secs(
                defaults.connect_timeout_seconds.unwrap_or(5).into()
            ),
            request: Duration::from_secs(defaults.timeout_seconds.into()),
            idle: Duration::from_secs(
                defaults.idle_timeout_seconds.unwrap_or(60).into()
            ),
        }
    }
}
```

### Updated Config Types

```rust
// src/config/types.rs - Additions

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub active: String,
    pub timeout_seconds: u32,
    /// Connection timeout in seconds (default: 5)
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_seconds: u32,
    /// Idle timeout for streaming in seconds (default: 60)
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_seconds: u32,
}

fn default_connect_timeout() -> u32 { 5 }
fn default_idle_timeout() -> u32 { 60 }
```

### Updated UpstreamClient

```rust
// src/proxy/upstream.rs - Key changes

use crate::proxy::error::ProxyError;
use crate::proxy::timeout::TimeoutConfig;
use tokio::time::timeout;

pub struct UpstreamClient {
    client: Client<HttpConnector, Full<Bytes>>,
    timeout_config: TimeoutConfig,
}

impl UpstreamClient {
    pub fn new(timeout_config: TimeoutConfig) -> Self {
        let connector = HttpConnector::new();
        // Note: Hyper client doesn't support per-request connect timeout directly
        // We'll use timeout wrapper around the entire request
        let client = Client::builder(TokioExecutor::new()).build(connector);
        
        Self { client, timeout_config }
    }

    pub async fn forward(
        &self, 
        req: Request<Incoming>, 
        config: Config
    ) -> Result<Response<UnsyncBoxBody<Bytes, hyper::Error>>, ProxyError> {
        // Resolve backend
        let active_backend_name = &config.defaults.active;
        let backend = config.backends
            .iter()
            .find(|b| &b.name == active_backend_name)
            .ok_or_else(|| ProxyError::BackendNotFound {
                backend: active_backend_name.clone()
            })?;

        // Validate backend configuration
        if !backend.is_configured() {
            return Err(ProxyError::BackendNotConfigured {
                backend: backend.name.clone(),
                reason: "api_key is not set".to_string()
            });
        }

        let upstream_uri = format!("{}{}", backend.base_url, path_and_query);

        // Build request with timeout
        let request_future = self.execute_request(req, upstream_uri, backend);
        
        let timeout_config = TimeoutConfig::from(&config.defaults);
        
        match timeout(timeout_config.request, request_future).await {
            Ok(result) => result,
            Err(_) => Err(ProxyError::RequestTimeout {
                duration: timeout_config.request.as_secs()
            })
        }
    }
    
    async fn execute_request(
        &self,
        req: Request<Incoming>,
        upstream_uri: String,
        backend: &Backend
    ) -> Result<Response<UnsyncBoxBody<Bytes, hyper::Error>>, ProxyError> {
        // ... existing request building logic ...
        
        let upstream_req = builder.body(Full::new(body_bytes))
            .map_err(|e| ProxyError::InvalidRequest(e.to_string()))?;

        let upstream_resp = self.client
            .request(upstream_req)
            .await
            .map_err(|e| ProxyError::ConnectionError {
                backend: backend.name.clone(),
                source: e
            })?;

        // Handle streaming with idle timeout
        if is_streaming {
            Ok(builder.body(
                IdleTimeoutBody::new(
                    upstream_resp.into_body(),
                    self.timeout_config.idle
                ).boxed_unsync()
            )?)
        } else {
            // ... existing non-streaming logic ...
        }
    }
}
```

### Error Response Builder

```rust
// src/proxy/error.rs

use hyper::{Response, StatusCode};
use http_body_util::Full;
use hyper::body::Bytes;

pub struct ErrorResponse;

impl ErrorResponse {
    pub fn from_error(err: &ProxyError, request_id: &str) -> Response<Full<Bytes>> {
        let body = serde_json::json!({
            "error": {
                "type": err.error_type(),
                "message": err.to_string(),
                "request_id": request_id
            }
        });

        Response::builder()
            .status(err.status_code())
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(body.to_string())))
            .unwrap()
    }
}
```

## Integration Points

### 1. RouterEngine Updates

```rust
// src/proxy/router.rs

use crate::proxy::error::{ProxyError, ErrorResponse};
use uuid::Uuid;

pub async fn route(&self, req: Request<Incoming>) -> Result<Response<UnsyncBoxBody<Bytes, hyper::Error>>> {
    let request_id = Uuid::new_v4().to_string();
    let path = req.uri().path();

    match (req.method(), path) {
        (&Method::GET, "/health") => self.health.handle().await,
        _ => {
            match self.upstream.forward(req, self.config.get()).await {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    tracing::error!(request_id = %request_id, error = %e, "Request failed");
                    Ok(ErrorResponse::from_error(&e, &request_id)
                        .map(|b| b.map_err(|never| match never {}).boxed_unsync()))
                }
            }
        }
    }
}
```

### 2. ProxyServer Updates

```rust
// src/proxy/mod.rs

use crate::proxy::timeout::TimeoutConfig;

impl ProxyServer {
    pub fn new(config: ConfigStore) -> Self {
        let addr = "127.0.0.1:8080".parse().expect("Invalid bind address");
        let timeout_config = TimeoutConfig::from(&config.get().defaults);
        let router = RouterEngine::new(config, timeout_config);
        // ...
    }
}
```

### 3. Cargo.toml Additions

```toml
[dependencies]
# Add to existing dependencies
uuid = { version = "1.0", features = ["v4"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "io-util", "signal", "time"] }
```

## Testing Strategy

### Unit Tests

1. **Error Type Tests**:
   - Verify each `ProxyError` variant maps to correct HTTP status
   - Verify error type strings are consistent

2. **Timeout Config Tests**:
   - Default values applied when not specified
   - Custom values parsed from config
   - Conversion from `Defaults` struct

3. **Error Response Tests**:
   - JSON format validation
   - Request ID inclusion
   - Content-Type header

### Integration Tests

1. **Timeout Tests**:
   - Request timeout triggers after configured duration
   - Idle timeout closes streaming connections
   - Connection timeout handled gracefully

2. **Error Handling Tests**:
   - Missing backend returns 502 with proper error type
   - Unconfigured backend returns 502 with reason
   - Invalid requests return 400
   - Upstream errors propagate correctly

## Acceptance Criteria

- [ ] `ProxyError` enum with all error variants defined
- [ ] Timeout configuration with connect/request/idle timeouts
- [ ] Config types updated with new timeout fields (backwards compatible)
- [ ] UpstreamClient enforces request timeouts
- [ ] Streaming responses use idle timeout
- [ ] Error responses are JSON with type, message, request_id
- [ ] All errors logged with request_id for tracing
- [ ] Unit tests for error types and timeout config
- [ ] Integration tests for timeout behavior
- [ ] Documentation updated with timeout configuration examples

## Migration Path

1. **Phase 1**: Add new error types and timeout config (no breaking changes)
2. **Phase 2**: Update UpstreamClient to use new error types
3. **Phase 3**: Implement timeout enforcement
4. **Phase 4**: Add request ID tracking and structured logging
5. **Phase 5**: Update integration tests

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Timeout changes break existing behavior | Configurable timeouts with same defaults |
| New error format breaks clients | Document as breaking change; clients should handle unknown fields |
| Performance overhead of UUID generation | UUID v4 is fast; can be optional in future |
| Timeout accuracy with tokio | Use tokio::time for accurate async timeouts |

## Open Questions

1. Should we add request ID to response headers for non-error responses?
2. Do we need configurable retry logic for connection failures?
3. Should we expose timeout metrics for monitoring?
4. Do we need different timeout configs per backend?
