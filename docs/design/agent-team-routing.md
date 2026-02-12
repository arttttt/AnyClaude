# Per-Agent Backend Routing for Agent Teams

**Date**: 2026-02-11
**Updated**: 2026-02-12
**Status**: Implemented (Phase 1) / In Progress (tmux shim)
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

### How Agent Teams Spawn (Empirically Verified)

Claude Code supports two teammate modes:

| Mode | `teammateMode` | How teammates run | Routing possible? |
|------|---------------|-------------------|-------------------|
| **In-process** | `"in-process"` (default) | All inside one process | No — no subprocess spawning |
| **Split panes** | `"tmux"` | Each teammate = separate `claude` process in tmux pane | Yes |

With `"auto"` (default), split panes are used only if already inside tmux.

**Critical finding**: in split-pane (tmux) mode, Claude Code uses `tmux send-keys`
with the **absolute path** to the `claude` binary:

```
tmux -L claude-swarm-26283 send-keys -t %0 \
  cd /path/to/project && \
  CLAUDECODE=1 CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1 \
  /opt/homebrew/Caskroom/claude-code/2.1.39/claude \
  --agent-id model-auditor@mvi-audit \
  --agent-name model-auditor \
  --team-name mvi-audit \
  --agent-color blue \
  --parent-session-id 0fc2e873-... \
  --agent-type Explore \
  --model claude-opus-4-6 Enter
```

This means:
- **PATH shim for `claude` is bypassed** — absolute path, no PATH lookup.
- **`CLAUDE_CODE_AGENT_TYPE` is NOT set as env var** — it's `--agent-type` CLI flag.
- **Only `CLAUDECODE=1` and `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` are set as env vars.**
- The tmux session is separate (`-L claude-swarm-{PID}`) — does not inherit parent env.

### Teammate tmux Protocol (Empirically Verified)

Full lifecycle captured via tmux shim logging:

1. `tmux -V` — version check
2. `tmux -L claude-swarm-{PID} new-session -d -s claude-swarm -n swarm-view -P -F #{pane_id}` — create detached session
3. For each teammate:
   a. `split-window -t %N -v/-h -P -F #{pane_id}` — create pane
   b. `select-pane -t %N -P bg=default,fg={color}` — styling
   c. `set-option -p -t %N pane-border-style fg={color}` — border
   d. `select-pane -t %N -T {agent-name}` — title
   e. `select-layout -t claude-swarm:swarm-view tiled` — layout
   f. **`send-keys -t %N cd /path && ENV_VARS /absolute/path/claude --agent-flags Enter`** — launch
4. Periodic `list-panes`, `has-session`, `list-windows` — health checks

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
  '-- PTY: PATH={shim_dir}:$PATH claude --teammate-mode tmux ...
             |
             '-- lead process (ANTHROPIC_BASE_URL=http://127.0.0.1:PORT)
                   |
                   '-- calls tmux send-keys with absolute claude path
                        |
                        '-- {shim_dir}/tmux (shim) intercepts
                             parses send-keys command
                             injects ANTHROPIC_BASE_URL=http://127.0.0.1:PORT/teammate
                             delegates to real tmux
```

**Why tmux shim, not claude shim?**

Claude Code resolves the absolute path to `claude` at startup and passes it
to `tmux send-keys`. A PATH-based `claude` shim is never found because there
is no PATH lookup — the absolute path bypasses it entirely.

The tmux shim intercepts the `send-keys` command and injects the teammate
`ANTHROPIC_BASE_URL` into the environment variables being typed into the pane.

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

## Component 2: PATH Shims

### Purpose

Make teammate processes send requests to `/teammate/v1/messages` instead of
`/v1/messages`, so the routing layer can distinguish them.

Two shims are placed in a temp directory prepended to PATH:

| Shim | Purpose | Status |
|------|---------|--------|
| `claude` | Defense-in-depth: rewrites `ANTHROPIC_BASE_URL` if `CLAUDE_CODE_AGENT_TYPE` is set | Implemented (but bypassed — see below) |
| `tmux` | Intercepts `send-keys` commands, injects `ANTHROPIC_BASE_URL` for teammates | Logging phase |

### Why claude shim alone doesn't work

Claude Code uses `tmux send-keys` with the **absolute path** to the binary
(e.g. `/opt/homebrew/Caskroom/claude-code/2.1.39/claude`). There is no PATH
lookup — our `claude` shim is never found.

The `claude` shim is kept as defense-in-depth for future Claude Code versions
that might use relative paths or different spawn mechanisms.

### tmux Shim — Logging Phase (Current)

The tmux shim currently logs all invocations and delegates to real tmux:

```bash
#!/bin/bash
# AnyClaude tmux shim — logging phase.

SHIM_DIR="$(cd "$(dirname "$0")" && pwd)"
LOG="$SHIM_DIR/tmux_shim.log"
echo "[$(date '+%H:%M:%S.%N')] tmux $*" >> "$LOG"

# Find real tmux, skipping our shim directory.
find_real_tmux() { ... }

REAL_TMUX="$(find_real_tmux)"
exec "$REAL_TMUX" "$@"
```

### tmux Shim — Routing Phase (Next)

The tmux shim will parse `send-keys` arguments and inject
`ANTHROPIC_BASE_URL` into the teammate launch command:

```bash
# Detect send-keys with claude invocation
if [[ "$1" == *"send-keys"* ]]; then
  # Find the last argument (the command being typed)
  # Inject ANTHROPIC_BASE_URL=http://127.0.0.1:PORT/teammate
  # before the claude binary path
fi
exec "$REAL_TMUX" "$@"
```

The `send-keys` command from Claude Code has a predictable format:
```
tmux -L claude-swarm-{PID} send-keys -t %N \
  cd /path && ENV1=val1 ENV2=val2 /abs/path/claude --flags Enter
```

The shim inserts `ANTHROPIC_BASE_URL=http://127.0.0.1:PORT/teammate` into
the env var list before the claude path.

### Module Structure

```
src/shim/
  mod.rs      — TeammateShim (owns temp dir, coordinates both shims)
  claude.rs   — claude shim script generation + resolve_real_claude()
  tmux.rs     — tmux shim script generation
```

```rust
// src/shim/mod.rs — self-contained, no dependencies on proxy/axum

pub struct TeammateShim {
    _dir: tempfile::TempDir,   // auto-cleanup on Drop
    dir_path: PathBuf,
}

impl TeammateShim {
    /// Create both shim scripts in a temp directory.
    pub fn create(proxy_port: u16) -> Result<Self>;

    /// PATH env var with shim dir prepended.
    pub fn path_env(&self) -> (String, String);

    /// Path to tmux shim log (for debugging).
    pub fn tmux_log_path(&self) -> PathBuf;
}
```

### Startup Integration

In `runtime.rs`, after the proxy port is known:

```rust
// Create shims if agent_teams is configured
let _teammate_shim = if config_store.get().agent_teams.is_some() {
    match TeammateShim::create(actual_addr.port()) {
        Ok(shim) => {
            app_log("runtime", &format!(
                "Agent team routing enabled, tmux log: {}",
                shim.tmux_log_path().display(),
            ));
            Some(shim)
        }
        Err(err) => { app_log("runtime", &format!("...disabled: {err}")); None }
    }
} else { None };

// PATH + --teammate-mode tmux injected at all 3 spawn points
let shim_env = _teammate_shim.as_ref().map(|s| vec![s.path_env()]).unwrap_or_default();
let teammate_cli_args = if _teammate_shim.is_some() {
    vec!["--teammate-mode".into(), "tmux".into()]
} else { vec![] };
```

`--teammate-mode tmux` forces Claude Code to use split-pane mode, ensuring
each teammate is a separate process that goes through our tmux shim.

---

## Implementation Plan

### Phase 1: Routing Layer + Shims (MVP) ✅ DONE

**Goal**: Generic routing layer in proxy. Teammate routing as first rule.
PATH shims for claude + tmux. `--teammate-mode tmux` injection.

#### Files Created

| File | Purpose |
|------|---------|
| `src/proxy/routing.rs` | `RoutingRule` trait, middleware, `PathPrefixRule`, `RoutedTo` |
| `src/shim/mod.rs` | `TeammateShim` — owns temp dir, coordinates both shims |
| `src/shim/claude.rs` | Claude shim script + `resolve_real_claude()` |
| `src/shim/tmux.rs` | tmux shim script (logging phase) |
| `tests/routing.rs` | 8 tests for routing layer |
| `tests/shim.rs` | 8 tests for both shims |

#### Files Modified

| File | Change |
|------|--------|
| `src/config/types.rs` | `AgentTeamsConfig` struct, `agent_teams: Option<AgentTeamsConfig>` field |
| `src/config/loader.rs` | Validation: `teammate_backend` must exist in `[[backends]]` |
| `src/proxy/mod.rs` | `pub mod routing;` |
| `src/proxy/router.rs` | `build_router()` accepts rules, applies layer |
| `src/proxy/router.rs` | `proxy_handler` reads `RoutedTo` from extensions |
| `src/proxy/server.rs` | Build routing rule from config, pass to `build_router` |
| `src/ui/runtime.rs` | Create shims, inject PATH + `--teammate-mode tmux` at 3 spawn points |
| `tests/config_loader.rs` | 3 tests for agent_teams validation |

#### Flow

1. AnyClaude starts, reads config
2. If `[agent_teams].teammate_backend` is set:
   a. Validate `teammate_backend` exists in `[[backends]]`
   b. Create `PathPrefixRule { prefix: "/teammate", backend }` internally
   c. Resolve real `claude` binary, generate both shim scripts in temp dir
   d. Pass `PATH=shim_dir:$PATH` via `extra_env`
   e. Pass `--teammate-mode tmux` via `extra_args`
3. Proxy starts with routing middleware layer (if rules exist)
4. Lead starts with `--teammate-mode tmux`, requests go to `/v1/messages`
   → no rule matches → active backend
5. Lead spawns teammates via tmux → tmux shim intercepts `send-keys`
   → (Phase 1b) injects `ANTHROPIC_BASE_URL` with `/teammate` prefix
   → teammate requests go to `/teammate/v1/messages`
   → `PathPrefixRule` strips prefix → routes to teammate backend

### Phase 1b: Smart tmux Shim 🔧 NEXT

**Goal**: tmux shim parses `send-keys` and injects `ANTHROPIC_BASE_URL`
so teammate traffic goes through the `/teammate` routing path.

**Input data** (from tmux shim log):
```
tmux -L claude-swarm-26283 send-keys -t %0 \
  cd /path && CLAUDECODE=1 CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1 \
  /abs/path/claude --agent-flags Enter
```

**Approach**: in the bash shim, scan args for `send-keys` subcommand.
If found, scan remaining args for a token that looks like an absolute
path to claude (contains `/claude`). Insert
`ANTHROPIC_BASE_URL=http://127.0.0.1:__PORT__/teammate` before it.
All other tmux commands — forward unchanged.

```bash
#!/bin/bash
SHIM_DIR="$(cd "$(dirname "$0")" && pwd)"
LOG="$SHIM_DIR/tmux_shim.log"
REAL_TMUX="$(find_real_tmux)"  # same as current shim

# Detect send-keys with claude invocation, inject ANTHROPIC_BASE_URL.
inject_args() {
  local out=()
  local found_send_keys=false
  local injected=false
  for arg in "$@"; do
    if [ "$arg" = "send-keys" ]; then
      found_send_keys=true
    fi
    # Inject before the absolute claude path
    if $found_send_keys && ! $injected && [[ "$arg" == /* ]] && [[ "$arg" == *claude* ]]; then
      out+=("ANTHROPIC_BASE_URL=http://127.0.0.1:__PORT__/teammate")
      injected=true
    fi
    out+=("$arg")
  done
  echo "${out[@]}"
}

# Log and execute
echo "[$(date '+%H:%M:%S.%N')] tmux $*" >> "$LOG"

MODIFIED_ARGS="$(inject_args "$@")"
exec "$REAL_TMUX" $MODIFIED_ARGS
```

**Risks**:
- Argument parsing in bash is fragile with spaces/special chars in paths.
  Mitigation: Claude Code paths are controlled (no spaces in homebrew paths).
- Claude Code might change `send-keys` format in future versions.
  Mitigation: if no claude path found, forward unchanged (graceful degradation).

**Verification**:
1. Launch with `[agent_teams]` configured
2. Check debug log for `[ROUTING] Routed POST /teammate/v1/messages -> backend=X`
3. Check tmux_shim.log for `ANTHROPIC_BASE_URL` injection
4. Check Ctrl+S status popup — should show requests on both backends

### Phase 1c: Synthetic tmux (Future)

**Goal**: remove real tmux dependency entirely. The tmux shim handles the
full teammate lifecycle: process spawning, state tracking, fake responses.

**Why**: tmux is not always installed. macOS doesn't ship it. Users
shouldn't need to `brew install tmux` just for agent teams routing.

**How it works**: the shim maintains internal state and returns fake
responses that satisfy Claude Code's expectations:

| tmux Command | Synthetic Response |
|---|---|
| `-V` | `tmux 3.4` (hardcoded version string) |
| `new-session -d -s NAME -n WIN -P -F #{pane_id}` | Print `%0`, track session |
| `split-window -t %N -v -P -F #{pane_id}` | Print `%{N+1}`, track pane |
| `has-session -t NAME` | Exit 0 if session tracked, 1 otherwise |
| `list-panes -t ... -F #{pane_id}` | Print tracked pane IDs, one per line |
| `list-windows -t ... -F #{window_name}` | Print tracked window name |
| `select-pane`, `set-option`, `select-layout` | No-op (exit 0) |
| `send-keys -t %N CMD Enter` | Extract CMD, spawn with modified env |
| `kill-session -t NAME` | Kill tracked processes, cleanup |

For `send-keys` with a claude command, the shim:
1. Parses the shell command (`cd /path && ENV=val /abs/claude --flags`)
2. Adds `ANTHROPIC_BASE_URL=http://127.0.0.1:PORT/teammate` to env
3. Spawns the process in background (`nohup ... &`)
4. Tracks the PID for cleanup on `kill-session`

**Complexity**: ~150-200 lines of bash. Main challenge is maintaining
pane ID state and process lifecycle across multiple shim invocations.
Statefile in `$SHIM_DIR/state.json` or simple lockfile-based tracking.

**When to implement**: after Phase 1b is validated end-to-end. Only if
tmux dependency proves problematic for users.

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
| `teammate_backend` not in `[[backends]]` | Validation error at config load (tested) |
| Teammate backend same as lead | Works, just no cost difference |
| Real `claude` not found in PATH | Non-fatal warning, routing disabled |
| Shim dir cleanup on crash | `tempfile::TempDir` auto-cleans on Drop; OS cleans on reboot |
| tmux not installed | tmux shim logs error, exits 127 — Claude Code falls back gracefully |
| Claude Code uses absolute path to claude | Expected behavior — tmux shim handles this, not claude shim |
| Multiple overrides match | Most specific prefix wins (longer prefix first) |
| Override references nonexistent backend | Caught at config validation |
| In-process teammate mode | Forced to tmux mode via `--teammate-mode tmux` CLI arg |

## Answered Questions

1. **Does the lead process have `CLAUDE_CODE_AGENT_TYPE` set?**
   ✅ **No.** `CLAUDE_CODE_AGENT_TYPE` is not set as an env var at all.
   Claude Code passes `--agent-type Explore` as a CLI flag to teammates.
   The only env vars set for teammates are `CLAUDECODE=1` and
   `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`.

2. **Does `ANTHROPIC_BASE_URL` with a path prefix work with Claude Code?**
   ✅ **Not yet tested** — requires working tmux shim (Phase 1b).
   Proxy routing layer is ready to handle `/teammate/v1/messages`.

3. **How does Claude Code spawn teammates?**
   ✅ **Via `tmux send-keys` with absolute path.** Not via direct process
   spawn or PATH lookup. See "Teammate tmux Protocol" section above.

4. **Does a PATH-based `claude` shim work?**
   ❌ **No.** Claude Code uses absolute path in `send-keys`, bypassing PATH.
   The tmux shim approach is required.

## Open Questions

1. **Thinking block compatibility for teammate backend.**
   If teammates use a non-Anthropic backend, thinking blocks need translation.
   AnyClaude already handles this via `thinking_compat` backend config.
   Should work out of the box if teammate backend has `thinking_compat = true`.

2. **Model override in teammate prompts.**
   Claude Code passes `--model claude-opus-4-6` to teammates. If the teammate
   backend doesn't support that model, the request may fail.
   **Mitigation**: A future routing rule could rewrite the model field.

3. **Synthetic tmux (no real tmux dependency).**
   The tmux shim could handle the full teammate lifecycle without real tmux:
   spawn processes directly, manage their I/O, report status. This would
   remove the tmux dependency but requires significant effort to replicate
   tmux's pane/session management that Claude Code expects.

---

## Cost Analysis

Example: 4-agent team, 1-hour session.

| Scenario | Lead | 3 Teammates | Total | Savings |
|----------|------|-------------|-------|---------|
| All Opus (Anthropic) | $5 | $15 | $20 | — |
| Lead Opus + Teammates Sonnet (OpenRouter) | $5 | $2.25 | $7.25 | 64% |
| Lead Opus + Teammates Haiku (OpenRouter) | $5 | $0.45 | $5.45 | 73% |

Assumes ~100k input + 30k output tokens per agent per hour.
