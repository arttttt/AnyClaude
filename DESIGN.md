# Config Integration for Upstream — Design Document

> **Historical design doc (point-in-time).** An early design for wiring the
> config system into the proxy. Parts of it diverged from what shipped — in
> particular there is **no IPC**: backend switching is in-process, and the proxy
> lives in `src/proxy/` (`server.rs` / `router.rs` / `connection.rs` / `pool.rs`),
> not an `UpstreamClient`. Kept for context. For the current architecture see
> [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Problem Statement

The `UpstreamClient` currently has a hardcoded upstream URL (`https://api.anthropic.com`). The proxy module needs to integrate with the config system to:
1. Use the active backend from configuration
2. Inject proper authentication headers
3. Support hot-reload of config changes
4. Eventually enable runtime backend switching via IPC

## Current State Analysis

### Existing Config Module (✅ Complete)
- `src/config/types.rs` - Config data structures (Backend, Config, Defaults)
- `src/config/loader.rs` - Config loading and validation
- `src/config/credentials.rs` - Credential resolution from env vars
- `src/config/auth.rs` - Auth header building (`build_auth_header`)
- `src/config/watcher.rs` - Config hot-reload with `ConfigStore`

### Existing Proxy Module (⚠️ Needs Integration)
- `src/proxy/mod.rs` - ProxyServer entry point
- `src/proxy/router.rs` - RouterEngine routes to HealthHandler or UpstreamClient
- `src/proxy/upstream.rs` - UpstreamClient has hardcoded URL, no auth

## Design Decisions

### Decision 1: Who Owns Config Access?

**Option A: ProxyServer holds ConfigStore and passes to RouterEngine**
- Pros: Simple, follows existing patterns
- Cons: ConfigStore cloned for each request (cheap for small Config)

**Option B: RouterEngine holds ConfigStore via Arc<RwLock>**
- Pros: Centralized config access
- Cons: Requires Arc/RwLock for shared access

**Selected: Option A** - Clone Config per request is cheap (small structs), simpler code.

### Decision 2: How to Handle Auth Headers?

**Option A: Build auth headers in UpstreamClient per request**
- Pros: Always uses latest credentials (hot-reload env vars)
- Cons: Slight per-request overhead

**Option B: Cache auth headers in RouterEngine**
- Pros: Faster per request
- Cons: Cache invalidation on config reload

**Selected: Option A** - Simpler, ensures fresh credentials on env var change.

### Decision 3: Backend Switching Strategy

**Phase 1 (Current scope):** Load active backend at request time
- Read active backend name from Config
- Look up backend in backends list
- Forward request to that backend's base_url

**Phase 2 (Future scope):** Runtime switching via IPC
- Listen for IPC commands to change active backend
- Update ConfigStore.active field
- No additional design needed now

### Decision 4: Error Handling

When active backend is misconfigured or missing:
- Return 502 Bad Gateway with descriptive error message
- Log error details
- Continue serving other requests (don't crash server)

## Implementation Plan

### Phase 1: Core Integration (cl-wisp-cacjb → cl-wisp-chvyy)

#### 1. Update `src/proxy/mod.rs`
- Add `config: ConfigStore` field to ProxyServer
- Change `ProxyServer::new()` to `ProxyServer::new(config: ConfigStore)`
- Pass ConfigStore to RouterEngine

```rust
pub struct ProxyServer {
    pub addr: SocketAddr,
    router: RouterEngine,
    shutdown: Arc<ShutdownManager>,
    config: ConfigStore,
}

impl ProxyServer {
    pub fn new(config: ConfigStore) -> Self {
        let addr = "127.0.0.1:8080".parse().expect("Invalid bind address");
        let router = RouterEngine::new(config.clone());
        Self {
            addr,
            router,
            shutdown: Arc::new(ShutdownManager::new()),
            config,
        }
    }
}
```

#### 2. Update `src/proxy/router.rs`
- Add `config: ConfigStore` field
- Pass config to UpstreamClient on each request

```rust
#[derive(Clone)]
pub struct RouterEngine {
    health: Arc<HealthHandler>,
    upstream: Arc<UpstreamClient>,
    config: ConfigStore,
}

impl RouterEngine {
    pub fn new(config: ConfigStore) -> Self {
        Self {
            health: Arc::new(HealthHandler::new()),
            upstream: Arc::new(UpstreamClient::new()),
            config,
        }
    }

    pub async fn route(&self, req: Request<Incoming>) -> Result<Response<UnsyncBoxBody<Bytes, hyper::Error>>> {
        let path = req.uri().path();
        match (req.method(), path) {
            (&Method::GET, "/health") => self.health.handle().await,
            _ => self.upstream.forward(req, self.config.get()).await,
        }
    }
}
```

#### 3. Update `src/proxy/upstream.rs`
- Change `forward()` to accept `config: Config` parameter
- Resolve active backend from config
- Build auth headers using `build_auth_header()`
- Forward to backend's base_url

```rust
impl UpstreamClient {
    pub async fn forward(
        &self,
        req: Request<Incoming>,
        config: Config,
    ) -> Result<Response<UnsyncBoxBody<Bytes, hyper::Error>>> {
        let active_backend_name = &config.defaults.active;
        let backend = config.backends.iter()
            .find(|b| &b.name == active_backend_name)
            .ok_or_else(|| anyhow::anyhow!("Active backend '{}' not found", active_backend_name))?;

        let upstream_uri = format!("{}{}", backend.base_url, path_and_query);

        let mut builder = Request::builder()
            .method(method)
            .uri(upstream_uri);

        for (name, value) in req.headers() {
            if name != HOST {
                builder = builder.header(name, value);
            }
        }

        // Inject auth header
        if let Some((name, value)) = build_auth_header(backend) {
            builder = builder.header(&name, &value);
        }

        // ... rest of request handling
    }
}
```

### Phase 2: Integration Tests (cl-wisp-bpks8)

Add tests to verify:
- Config-loaded backends are used correctly
- Auth headers are injected
- Missing active backend returns 502
- Multiple backends can be switched between

### Phase 3: Update Main Entry Point

Update `src/ui/runtime.rs` or wherever ProxyServer is instantiated to:
- Load config at startup
- Pass ConfigStore to ProxyServer::new()
- Initialize ConfigWatcher with ConfigReload events

## Architecture Diagram

```
┌─────────────────┐
│   UI Runtime    │
└────────┬────────┘
         │
         │ loads Config
         ▼
┌─────────────────┐
│  ConfigStore   │ ◄─── ConfigWatcher (hot-reload)
└────────┬────────┘
         │
         │ passes to
         ▼
┌─────────────────┐
│  ProxyServer   │
└────────┬────────┘
         │
         │ clones per request
         ▼
┌─────────────────┐
│  RouterEngine  │ ── clones Config ──┐
└────────┬────────┘                 │
         │                          │
         │ route()                   │
         ▼                          │
┌─────────────────┐                 │
│ UpstreamClient  │ ◄───────────────┘
│                 │
│ - resolve backend
│ - build auth header
│ - forward request
└─────────────────┘
```

## Acceptance Criteria

- [ ] UpstreamClient uses backend.base_url from config, not hardcoded URL
- [ ] Auth headers (x-api-key or Bearer) are injected from env vars
- [ ] Missing/misconfigured active backend returns 502 error
- [ ] Config hot-reload works (changing active backend affects new requests)
- [ ] All existing tests pass
- [ ] New integration tests cover config-driven routing

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Config clone overhead per request | Config is small (few backends), clone is cheap |
| Auth header lookup per request | Build header once, negligible overhead |
| Race condition on hot-reload | RwLock in ConfigStore ensures atomic reads |
| Backend switch mid-request | Each request uses config snapshot, safe |

## Open Questions

None for Phase 1. IPC integration for runtime switching deferred to future work.
