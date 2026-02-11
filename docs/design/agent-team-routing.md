# Per-Agent Backend Routing for Agent Teams

**Date**: 2026-02-11
**Status**: Draft / RFC
**Parent**: [Agent Teams Integration (Ctrl+T)](agent-teams-integration.md)

## Problem

When Claude Code spawns an Agent Team (1 lead + N teammates), every agent
makes API requests through the same backend with the same API key. A 4-agent
team on Opus costs ~$10-20 per session. Most teammate work (code review,
testing, simple refactors) doesn't need a frontier model.

## Goal

Route lead requests through an expensive backend (Anthropic/Opus) and
teammate requests through a cheap backend (OpenRouter/Sonnet), transparently.
No changes to Claude Code internals.

## Background

### How AnyClaude Already Works

AnyClaude spawns Claude Code as a child process with:

```
ANTHROPIC_BASE_URL=http://127.0.0.1:{PORT}
```

All API requests go through our local proxy, which forwards them to the
configured backend. This is how we already support multi-backend switching
(Ctrl+B), metrics, and thinking block management.

### How Agent Teams Spawn

Claude Code's lead process spawns teammates as child processes. Each teammate
inherits the parent's environment, including `ANTHROPIC_BASE_URL`. This means
**all teammate traffic already flows through our proxy** — we just can't
distinguish it from the lead's traffic.

### Teammate Environment Variables

Confirmed from Claude Code binary analysis and official docs. Each teammate
process has these env vars set before exec:

| Variable | Example | Purpose |
|----------|---------|---------|
| `CLAUDE_CODE_TEAM_NAME` | `"debug-session"` | Team namespace |
| `CLAUDE_CODE_AGENT_ID` | `"abc-123"` | Unique agent identifier |
| `CLAUDE_CODE_AGENT_NAME` | `"investigator-a"` | Display name |
| `CLAUDE_CODE_AGENT_TYPE` | `"teammate"` | Role (lead has no type or different value) |

### Prior Art: HydraTeams

HydraTeams is a standalone proxy that solves the same problem. It detects
teammates via "hidden marker in CLAUDE.md" — a fragile hack. Our approach
is cleaner because we control the process spawn chain.

### Current Proxy Architecture Gap

The proxy has no routing layer. Backend selection is either global
(`BackendState::get_active_backend()`, toggled via Ctrl+B) or via
`ObservabilityPlugin::pre_request()` which returns `Option<BackendOverride>`.
The plugin mechanism is semantically wrong for routing — it was designed for
observability (logging, metrics), and no plugin actually uses the override
(both `DebugLogger` and `RequestParser` return `None`).

There is no way to route requests to different backends based on request
properties (path, headers, body).

---

## Design

Two independent components:

1. **Routing Layer** — generic proxy middleware for rule-based backend routing.
   Not teammate-specific. Teammate routing is one concrete rule.
2. **PATH Shim** — intercepts teammate process spawning to tag their requests
   with a different URL prefix.

### Architecture

```
AnyClaude
  |
  +-- Proxy :PORT
  |     |
  |     +-- RoutingLayer (Axum middleware)
  |     |     evaluates rules → sets RoutedTo extension
  |     |     strips path prefix if needed
  |     |
  |     +-- proxy_handler (reads RoutedTo, forwards to correct backend)
  |
  '-- PTY: PATH={shim_dir}:$PATH claude ...
             |
             '-- lead process (ANTHROPIC_BASE_URL=http://127.0.0.1:PORT)
                   |
                   '-- spawns teammate
                        |
                        '-- {shim_dir}/claude (shim)
                             sees CLAUDE_CODE_AGENT_TYPE != ""
                             sets ANTHROPIC_BASE_URL=http://127.0.0.1:PORT/teammate
                             exec {real_claude} "$@"
```

Request flow:

```
Lead request:     POST /v1/messages
                  → RoutingLayer: no rule matches → no extension
                  → proxy_handler: no RoutedTo → active backend
                  → upstream: Anthropic (Opus)

Teammate request: POST /teammate/v1/messages
                  → RoutingLayer: PathPrefixRule matches
                    → strips "/teammate", rewrites URI to /v1/messages
                    → inserts RoutedTo { backend: "openrouter-sonnet" }
                  → proxy_handler: reads RoutedTo → override backend
                  → upstream: OpenRouter (Sonnet)
```

---

## Component 1: Routing Layer

### Abstraction

```rust
// src/proxy/routing.rs

/// Inserted into request extensions by the routing middleware.
/// Read by proxy_handler to determine the backend.
pub struct RoutedTo {
    pub backend: String,
    pub reason: String,
}

/// A routing rule. Rules are evaluated in order; first match wins.
pub trait RoutingRule: Send + Sync {
    fn evaluate(&self, req: &Request<Body>) -> Option<RouteAction>;
}

pub struct RouteAction {
    /// Backend name (must exist in [[backends]]).
    pub backend: String,
    /// Human-readable reason for logging/metrics.
    pub reason: String,
    /// Path prefix to strip before forwarding.
    pub strip_prefix: Option<String>,
}
```

One trait, one result type, one extension point. New routing rules are new
implementations of `RoutingRule`, with zero changes to existing code.

### Middleware

```rust
/// Axum middleware. Applied as a layer on the Router.
async fn routing_middleware(
    Extension(rules): Extension<Arc<Vec<Box<dyn RoutingRule>>>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    for rule in rules.iter() {
        if let Some(action) = rule.evaluate(&req) {
            if let Some(prefix) = &action.strip_prefix {
                rewrite_uri(&mut req, prefix);
            }
            req.extensions_mut().insert(RoutedTo {
                backend: action.backend,
                reason: action.reason,
            });
            break;
        }
    }
    next.run(req).await
}
```

The middleware modifies the request **before** `proxy_handler` sees it.
The handler doesn't know about rules — it only reads the result.

When no rules are configured, the middleware layer is not applied. Zero
overhead for existing users.

### proxy_handler Change

The only change to existing business logic — 3 lines:

```rust
// Before:
let active_backend = state.backend_state.get_active_backend();

// After:
let active_backend = req.extensions()
    .get::<routing::RoutedTo>()
    .map(|r| r.backend.clone())
    .unwrap_or_else(|| state.backend_state.get_active_backend());
```

If no `RoutedTo` extension — behavior is identical to current code.

### Concrete Rule: PathPrefixRule

```rust
pub struct PathPrefixRule {
    pub prefix: String,
    pub backend: String,
}

impl RoutingRule for PathPrefixRule {
    fn evaluate(&self, req: &Request<Body>) -> Option<RouteAction> {
        if req.uri().path().starts_with(&self.prefix) {
            Some(RouteAction {
                backend: self.backend.clone(),
                reason: format!("path prefix {}", self.prefix),
                strip_prefix: Some(self.prefix.clone()),
            })
        } else {
            None
        }
    }
}
```

`PathPrefixRule` knows nothing about teammates. It's generic: "if path
starts with X, strip it and route to backend Y." Teammate routing is one
instance with `prefix = "/teammate"`.

### Router Composition

```rust
pub fn build_router(
    engine: RouterEngine,
    rules: Vec<Box<dyn RoutingRule>>,
) -> Router {
    let mut router = Router::new()
        .route("/health", get(health_handler))
        .fallback(proxy_handler)
        .with_state(engine);

    if !rules.is_empty() {
        router = router
            .layer(Extension(Arc::new(rules)))
            .layer(axum::middleware::from_fn(routing_middleware));
    }

    router
}
```

No rules — no layer. Existing behavior preserved exactly.

### Configuration

The config is domain-specific — the user says **what** they want (teammates
on a cheaper backend), not **how** it works (path prefixes, routing rules).
Internal translation from config to routing rules is an implementation detail.

```toml
# ~/.config/anyclaude/config.toml

# Existing backends (already configured by user)
[[backends]]
name = "anthropic"
display_name = "Anthropic"
base_url = "https://api.anthropic.com"
auth_type = "passthrough"

[[backends]]
name = "openrouter-sonnet"
display_name = "OpenRouter Sonnet"
base_url = "https://openrouter.ai/api/v1"
auth_type = "bearer"
api_key = "sk-or-..."

# Agent Teams — one field
[agent_teams]
teammate_backend = "openrouter-sonnet"
```

That's it. `teammate_backend` is the name of a backend from `[[backends]]`.
When not set (or `[agent_teams]` absent), all agents use the active backend —
current behavior, zero overhead.

Internally, this creates a `PathPrefixRule { prefix: "/teammate", backend }`.
The user never sees routing rules, path prefixes, or middleware details.

---

## Component 2: PATH Shim

### Purpose

Make teammate processes send requests to `/teammate/v1/messages` instead of
`/v1/messages`, so the routing layer can distinguish them.

The shim is a `claude` wrapper script placed first in PATH. When Claude Code's
lead process spawns a teammate, the OS finds our shim first. The shim checks
for teammate env vars, modifies `ANTHROPIC_BASE_URL` to add the `/teammate`
prefix, and execs the real `claude` binary.

### Shim Script

Generated at runtime into a temp directory:

```bash
#!/bin/bash
# AnyClaude routing shim.
# Intercepts Claude Code subprocess spawns to modify
# ANTHROPIC_BASE_URL based on environment variables.

if [ -n "$CLAUDE_CODE_AGENT_TYPE" ]; then
  export ANTHROPIC_BASE_URL="http://127.0.0.1:__PORT__/teammate"
fi

exec "__REAL_CLAUDE__" "$@"
```

AnyClaude replaces `__PORT__` and `__REAL_CLAUDE__` before writing the file.

`__REAL_CLAUDE__` is resolved at startup by scanning PATH (excluding the
shim directory) for the real `claude` binary.

### Implementation

```rust
// src/shim.rs — self-contained, no dependencies on proxy/axum

pub struct TeammateShim {
    _dir: tempfile::TempDir,   // auto-cleanup on Drop
    dir_path: PathBuf,
}

impl TeammateShim {
    /// Create shim script in a temp directory.
    pub fn create(proxy_port: u16) -> Result<Self>;

    /// PATH env var value with shim dir prepended.
    /// For use with PtySpawnConfig::build(extra_env).
    pub fn path_env(&self) -> (String, String);
}

fn resolve_real_claude() -> Result<PathBuf>;
```

The shim module has no dependency on the proxy, routing, or axum. It is
purely a filesystem/process concern: write a script, resolve a binary path,
provide a PATH env var.

### Startup Integration

In `runtime.rs`, after the proxy port is known:

```rust
// Create shim if routing rules reference "/teammate" prefix
let shim = if has_teammate_routing(&config) {
    Some(TeammateShim::create(actual_addr.port())?)
} else {
    None
};

// Pass PATH via extra_env — no changes to PtySpawnConfig
let mut initial_env = app.settings_manager().to_env_vars();
if let Some(s) = &shim {
    initial_env.push(s.path_env());
}
let initial = spawn_config.build(initial_env, initial_args, SessionMode::Initial);
// `shim` lives until end of run() — TempDir is not dropped prematurely
```

---

## Implementation Plan

### Phase 1: Routing Layer + Shim (MVP)

**Goal**: Generic routing layer in proxy. Teammate routing as first rule.

#### New Files

| File | Purpose | ~Lines |
|------|---------|--------|
| `src/proxy/routing.rs` | `RoutingRule` trait, middleware, `PathPrefixRule`, `RoutedTo` | 80 |
| `src/shim.rs` | `TeammateShim`, shim script generation, resolve real claude | 50 |

#### Modified Files

| File | Change | ~Lines |
|------|--------|--------|
| `src/config/types.rs` | Add `AgentTeamsConfig` struct, `agent_teams: Option<AgentTeamsConfig>` field | +10 |
| `src/proxy/mod.rs` | `pub mod routing;` | +1 |
| `src/proxy/router.rs` | `build_router()` accepts rules, applies layer | +5 |
| `src/proxy/router.rs` | `proxy_handler` reads `RoutedTo` from extensions | +3 |
| `src/proxy/server.rs` | Build routing rule from `agent_teams` config, pass to `build_router` | +8 |
| `src/ui/runtime.rs` | Create shim, pass PATH via `extra_env` | +8 |

#### Unchanged

- `upstream.rs` — zero changes
- `ObservabilityHub` — zero changes
- `ObservabilityPlugin` — zero changes
- `PtySpawnConfig` — zero changes (PATH goes via existing `extra_env`)
- `PtySession` — zero changes
- `BackendState` — zero changes

#### Flow

1. AnyClaude starts, reads config
2. If `[agent_teams].teammate_backend` is set:
   a. Create `PathPrefixRule { prefix: "/teammate", backend }` internally
   b. Resolve real `claude` binary path
   c. Generate shim script in temp dir
   d. Pass `PATH=shim_dir:$PATH` via `extra_env`
3. Proxy starts with routing middleware layer (if rules exist)
4. Lead process starts, requests go to `/v1/messages` → no rule matches
   → active backend
5. Lead spawns teammate → teammate runs through shim → requests go to
   `/teammate/v1/messages` → `PathPrefixRule` matches → strips prefix
   → forwards `/v1/messages` to teammate backend

### Phase 2: Per-Agent Metrics (Free)

Metrics are already aggregated per-backend via `ObservabilityHub.snapshot()`.
If teammate requests go through backend `"openrouter-sonnet"`, they
automatically appear as separate metrics. Zero additional code.

Status popup (Ctrl+S) already shows per-backend breakdown:

```
anthropic:          $1.80  (32 req)
openrouter-sonnet:  $0.54  (15 req)
Total:              $2.34  (47 req)
```

### Phase 3: Per-Agent / Per-Team Routing (Optional)

Different backends per agent or per team, not just lead vs all teammates.

Each teammate process has env vars identifying it:

| Variable | Example | Granularity |
|----------|---------|-------------|
| `CLAUDE_CODE_AGENT_NAME` | `"investigator-a"` | Individual agent |
| `CLAUDE_CODE_TEAM_NAME` | `"debug-session"` | Team namespace |

The shim encodes these into the URL path:

```
/teammate/v1/messages                           — basic (Phase 1)
/teammate/{agent-name}/v1/messages              — per-agent
/teammate/{team-name}/{agent-name}/v1/messages  — per-team + per-agent
```

Config uses an `overrides` map — agent or team name to backend:

```toml
[agent_teams]
teammate_backend = "openrouter-sonnet"   # default for all teammates

[agent_teams.overrides]
architect = "anthropic"              # agent named "architect" gets Opus
test-runner = "openrouter-haiku"     # agent named "test-runner" gets cheapest
```

Internally, each override creates a `PathPrefixRule` with a more specific
prefix (e.g., `/teammate/architect`). More specific prefixes are evaluated
before the catch-all `/teammate` rule. This works with `PathPrefixRule`
alone — no new rule type needed.

The shim decides what to encode based on config: if only `teammate_backend`
is set, just `/teammate`. If `overrides` exist, encode the agent name too.

---

## Edge Cases

| Case | Handling |
|------|----------|
| No `[agent_teams]` in config | No middleware applied, zero overhead, current behavior |
| `teammate_backend` not in `[[backends]]` | Validation error at config load |
| Teammate backend same as lead | Works, just no cost difference |
| Real `claude` not found in PATH | Error at startup with clear message |
| Shim dir cleanup on crash | `tempfile::TempDir` auto-cleans on Drop; OS cleans on reboot |
| Claude Code updates change spawn mechanism | Shim is a no-op if env vars aren't set; graceful degradation |
| Lead process has `CLAUDE_CODE_AGENT_TYPE` | Verify empirically; if so, shim checks for specific value `"teammate"` |
| Multiple overrides match | Most specific prefix wins (longer prefix first) |
| Override references nonexistent backend | Caught at config validation |

## Open Questions

1. **Does the lead process have `CLAUDE_CODE_AGENT_TYPE` set?**
   If yes, we need to distinguish by value (e.g., `"lead"` vs `"teammate"`).
   If no, the simple `[ -n "$CLAUDE_CODE_AGENT_TYPE" ]` check works.
   **Action**: Test empirically with `env | grep CLAUDE_CODE` in both contexts.

2. **Does `ANTHROPIC_BASE_URL` with a path prefix work with Claude Code?**
   Claude Code likely appends `/v1/messages` to the base URL. If the base URL
   is `http://127.0.0.1:PORT/teammate`, requests go to
   `http://127.0.0.1:PORT/teammate/v1/messages`. Need to verify.
   **Action**: Test with a simple proxy that logs request paths.

3. **Thinking block compatibility for teammate backend.**
   If teammates use a non-Anthropic backend, thinking blocks need translation.
   AnyClaude already handles this via `thinking_compat` backend config.
   Should work out of the box if teammate backend has `thinking_compat = true`.

4. **Model override in teammate prompts.**
   Users can ask the lead to "use Sonnet for teammates" — Claude Code may
   set a model field in the API request. If the teammate backend doesn't
   support that model name, the request fails.
   **Mitigation**: A future routing rule could rewrite the model field.

---

## Cost Analysis

Example: 4-agent team, 1-hour session.

| Scenario | Lead | 3 Teammates | Total | Savings |
|----------|------|-------------|-------|---------|
| All Opus (Anthropic) | $5 | $15 | $20 | — |
| Lead Opus + Teammates Sonnet (OpenRouter) | $5 | $2.25 | $7.25 | 64% |
| Lead Opus + Teammates Haiku (OpenRouter) | $5 | $0.45 | $5.45 | 73% |

Assumes ~100k input + 30k output tokens per agent per hour.
