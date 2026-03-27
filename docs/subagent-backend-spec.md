# Subagent Backend Selection for the Main Client

## Context

Claude Code supports the env var `CLAUDE_CODE_SUBAGENT_MODEL` — it overrides the model for **all** subagents of the process. CC **does not propagate** this variable to teammates (the `h28` array in the binary only contains `CLAUDE_CODE_USE_BEDROCK/VERTEX/FOUNDRY`, `ANTHROPIC_BASE_URL`, `CLAUDE_CONFIG_DIR`).

AnyClaude already has `detect_marker_model()` (`src/proxy/pipeline/routing.rs`) — it detects `marker-` and `anyclaude-` prefixes in the model name and routes to the corresponding backend. `model_map` on backends also works — it rewrites the model before forwarding upstream.

**Goal:** allow the user to choose a backend for subagents of the main client (not teammates).

**Mechanism:** SubagentStart hook injects AC marker `⟨AC:backend_name⟩` via `additionalContext` → marker appears in subagent's request body → `resolve_backend()` extracts marker from body → routes to that backend → `transform` rewrites the model via backend's `model_map`. Changing subagent backend = updating hook response, **no PTY restart**.

### How CC Chooses the Subagent Model (reverse engineering v2.1.50+)

```js
function getSubagentModel(config, parentModel, frontmatterModel, permissionMode) {
  if (process.env.CLAUDE_CODE_SUBAGENT_MODEL)
    return parse(process.env.CLAUDE_CODE_SUBAGENT_MODEL);  // env var overrides everything
  if (frontmatterModel) return parse(frontmatterModel);     // model in agent .md
  if (config === "inherit")
    return getRuntimeMainLoopModel({ permissionMode, mainLoopModel: parentModel });
  return parse(config);
}
```

**Key insight:** Without `CLAUDE_CODE_SUBAGENT_MODEL`, CC uses its own default (haiku) for subagents. The AC marker handles routing independently of the model name.

### What CC Passes to Teammates on Spawn (tmux send-keys)

**Always:**
- `CLAUDECODE=1`
- `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`

**If set (from `h28`):**
- `CLAUDE_CODE_USE_BEDROCK`
- `CLAUDE_CODE_USE_VERTEX`
- `CLAUDE_CODE_USE_FOUNDRY`
- `ANTHROPIC_BASE_URL`
- `CLAUDE_CONFIG_DIR`

`CLAUDE_CODE_SUBAGENT_MODEL` **is not in the list** → teammates will not receive this variable → their subagents will use default routing (active backend).

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                     AnyClaude (main)                             │
│                                                                  │
│  config.toml:                                                    │
│    [agent_teams]                                                 │
│    teammate_backend = "cheap-api"                                │
│    subagent_backend = "cheap-api"  ← initial value               │
│                                                                  │
│  ArgAssembler: --settings with SubagentStart/SubagentStop hooks  │
│                                                                  │
│  SubagentBackend (Arc<RwLock<Option<String>>>)                   │
│    └─ shared runtime state, read by SubagentStart hook           │
│    └─ initialized from config.subagent_backend on start          │
│                                                                  │
│  ┌──────────────────────────┐                                    │
│  │  Claude Code (PTY)       │                                    │
│  │  env: no special vars    │                                    │
│  │  hooks: SubagentStart    │                                    │
│  │        → inject marker   │                                    │
│  │                          │                                    │
│  │  Main agent → proxy      │──→ active backend (routing.rs)     │
│  │  Subagent  → proxy       │──→ extract AC marker from body →   │
│  │                          │    route to backend from marker    │
│  │  Teammate  → /teammate   │──→ cheap-api (BackendOverride)     │
│  │    └─ Subagent → proxy   │──→ active backend (no marker)      │
│  └──────────────────────────┘                                    │
└──────────────────────────────────────────────────────────────────┘
```

**Subagent request flow:**
1. CC spawns subagent → SubagentStart hook fires
2. Hook reads `SubagentBackend` shared state → returns `additionalContext: "⟨AC:cheap-api⟩"`
3. CC injects marker into subagent's context as `<system-reminder>`
4. Subagent makes API request: `POST /v1/messages {"model": "claude-haiku-4-5-20251001", ...}`
5. `routing.rs::resolve_backend()` → extracts AC marker from body → routes to `cheap-api`
6. `transform.rs` → `backend.resolve_model("claude-haiku-4-5-20251001")` → model_map
7. `headers.rs` → auth headers for `cheap-api` backend
8. `forward.rs` → request goes to upstream URL of `cheap-api` backend

---

## Routing Priority

`resolve_backend()` checks in order:
1. `plugin_override` — observability plugin routing
2. `backend_override` — teammate routes
3. `ac_marker_backend` — AC marker from request body (session affinity)
4. `marker_backend` — `marker-*` / `anyclaude-*` prefixes in model name
5. `active_backend` — default

---

## Changes by File

### 1. Config: add `subagent_backend` field

**File:** `src/config/types.rs` (line ~249)

```rust
/// Agent Teams routing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsConfig {
    /// Backend name for teammate requests (must exist in [[backends]]).
    pub teammate_backend: String,
    /// Backend for subagents of the main client (optional).
    /// Used as initial value for SubagentBackend runtime state.
    /// Does NOT affect teammates — CC does not propagate this env var.
    #[serde(default)]
    pub subagent_backend: Option<String>,
}
```

**Example config.toml:**
```toml
[agent_teams]
teammate_backend = "openrouter"
subagent_backend = "openrouter"   # optional, if omitted — inherits parent model
```

### 2. Config validation

**File:** `src/config/loader.rs` (after line ~141, inside `if let Some(ref at) = self.agent_teams`)

```rust
if let Some(ref sb) = at.subagent_backend {
    if !self.backends.iter().any(|b| b.name == *sb) {
        return Err(ConfigError::ValidationError {
            message: format!(
                "agent_teams.subagent_backend '{}' not found in configured backends",
                sb
            ),
        });
    }
}
```

### 3. Shared state: `SubagentBackend`

**File:** `src/backend/state.rs`

```rust
use std::sync::{Arc, RwLock};

/// Runtime state for subagent backend routing.
/// Initialized from config on startup, read by SubagentStart hook.
#[derive(Clone)]
pub struct SubagentBackend {
    inner: Arc<RwLock<Option<String>>>,
}

impl SubagentBackend {
    pub fn new(initial: Option<String>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(initial)),
        }
    }

    /// Get current subagent backend name.
    pub fn get(&self) -> Option<String> {
        self.inner.read().unwrap().clone()
    }

    /// Set subagent backend. None = disable (inherit parent model).
    pub fn set(&self, backend: Option<String>) {
        *self.inner.write().unwrap() = backend;
    }
}
```

### 4. Routing: AC marker extraction

**File:** `src/proxy/pipeline/routing.rs`

`resolve_backend()` extracts AC marker from request body independently of model name:

```rust
pub fn resolve_backend(
    backend_state: &BackendState,
    _subagent_backend: &SubagentBackend,
    backend_override: Option<String>,
    plugin_override: Option<BackendOverride>,
    parsed_body: Option<&Value>,
    ctx: &mut PipelineContext,
) -> Result<Backend, ProxyError> {
    // Extract AC marker from request body (session affinity from hook)
    let ac_marker_backend = parsed_body.and_then(extract_ac_marker);

    // Check for marker model in request body
    let marker_backend = parsed_body
        .and_then(|body| body.get("model"))
        .and_then(|m| m.as_str())
        .and_then(|model| detect_marker_model(model, backend_state));

    // Priority: plugin_override > backend_override > ac_marker_backend > marker_backend > active_backend
    let backend_id = plugin_override
        .as_ref()
        .map(|o| o.backend.clone())
        .or(backend_override.clone())
        .or(ac_marker_backend.clone())
        .or(marker_backend.clone())
        .unwrap_or(active_backend);
    // ...
}
```

`extract_ac_marker()` searches for `⟨AC:backend_name⟩` in the request body.

### 5. ArgAssembler: hook injection

**File:** `src/args/assembler.rs`

```rust
pub fn with_subagent_hooks(mut self, proxy_port: u16) -> Self {
    let hooks_json = format!(
        r#"{{"hooks":{{"SubagentStart":[{{"matcher":"","hooks":[{{"type":"command","command":"curl -s -m 5 -X POST http://127.0.0.1:{port}/api/subagent-start -d @- -H 'Content-Type: application/json'"}}]}}],"SubagentStop":[{{"matcher":"","hooks":[{{"type":"command","command":"curl -s -m 5 -X POST http://127.0.0.1:{port}/api/subagent-stop -d @- -H 'Content-Type: application/json'"}}]}}]}}}}"#,
        port = proxy_port
    );
    self.args.push("--settings".into());
    self.args.push(hooks_json);
    self
}
```

---

## Files That Do NOT Need Changes

| File | Reason |
|------|--------|
| `src/proxy/pipeline/transform.rs` | `model_map` on backends already rewrites the model |
| `src/proxy/router.rs` | Routing via main pipeline, no new route needed |
| `src/shim/tmux.rs` | No shim injection needed (main client only) |
| `src/ipc/` | No new IPC commands needed (subagent_backend is local state) |
| `src/args/env_builder.rs` | No `CLAUDE_CODE_SUBAGENT_MODEL` env var needed |

---

## Verification

1. **Config:** add `subagent_backend = "openrouter"` to `~/.config/anyclaude/config.toml` → AnyClaude starts without errors
2. **Hook fires:** create subagent (Task tool) → proxy log shows `POST /api/subagent-start`
3. **Routing:** proxy debug log shows: `routing_decision: { backend: "openrouter", reason: "ac marker session affinity" }`
4. **Model rewrite:** in forwarded request to upstream — model is rewritten via `model_map`
5. **UI:** Ctrl+B → change subagent backend → **no PTY restart**, next subagent request routes to new backend
6. **Session affinity:** existing subagent continues on old backend (marker baked in at start)
7. **Teammates:** check in shim log (`~/.config/anyclaude/logs/tmux_shim.log`) that `CLAUDE_CODE_SUBAGENT_MODEL` is NOT present
8. **Tests:** `cargo test` — all existing tests pass
9. **Validation:** `subagent_backend = "nonexistent"` → error on startup

---

## Limitations and Edge Cases

1. **Main client only** — teammates do not receive the env var (by design)
2. **`model_map` is required on the target backend** — if `model_map` is not set, upstream will receive the model name as-is and may return an error
3. **frontmatter `model:` in agents** — CC uses its default (haiku) for subagents without `CLAUDE_CODE_SUBAGENT_MODEL`
4. **Thread safety** — `SubagentBackend` uses `Arc<RwLock<...>>`, safe for concurrent reads from proxy threads and writes from UI thread
5. **Enterprise `allowManagedHooksOnly`** — may block hook injection; SubagentBackend state still works for display purposes
