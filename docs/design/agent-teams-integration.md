# Agent Teams Integration (Ctrl+T)

**Date**: 2026-02-08
**Status**: Draft / RFC
**Depends on**: [Settings Menu (Ctrl+E)](settings-menu.md)
**See also**: [Per-Agent Backend Routing](agent-team-routing.md) — detailed design for routing teammates through cheaper backends

## Motivation

Claude Code shipped experimental Agent Teams — a multi-agent collaboration model
where 3–5 independent Claude Code instances collaborate on the same project with
shared task lists, direct messaging, and explicit lifecycle control.

Currently, Agent Teams require tmux or iTerm2 for multi-pane display. AnyClaude's
PTY + proxy architecture uniquely positions us to:

1. Replace tmux/iTerm2 with native multi-pane TUI
2. Route each teammate through a different backend (cost optimization)
3. Provide a unified dashboard for team status, tasks, and metrics

## Prerequisites

- Settings Menu (Ctrl+E) implemented — enables `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS`
  toggle and PTY restart with `--continue`
- PTY restart infrastructure — graceful shutdown + respawn

---

## Background: How Agent Teams Work

### Internal Tools

Claude Code Agent Teams use five internal tools:

| Tool | Purpose |
|------|---------|
| **TeamCreate** | Creates team scaffolding (`.claude/teams/<team_id>/`) |
| **TaskCreate** | Adds tasks as JSON files with status, dependencies, ownership |
| **Task (upgraded)** | Spawns agents with `name` and `team_name` params for team mode |
| **taskUpdate** | Agents claim tasks, update status, mark done |
| **sendMessage** | Direct messages (agent↔agent) and broadcasts (agent→all) |

### File System Layout

```
~/.claude/teams/
  └── <team-id>/
      ├── config.json          # Team configuration (members, roles)
      └── inbox/
          ├── <agent-id>.json  # Messages to specific agent
          └── broadcast.json   # Broadcast messages

~/.claude/tasks/
  └── <team-name>/
      ├── task-001.json        # Task definitions with status tracking
      └── task-002.json
```

### Teammate Modes

| Mode | Behavior |
|------|----------|
| `auto` | tmux panes if inside tmux, in-process otherwise |
| `in-process` | All teammates in main terminal, Shift+Up/Down to select |
| `tmux` | Each teammate gets its own tmux/iTerm2 pane |

### Lifecycle

1. Team lead creates team via prompt (e.g., "Create a team to investigate this bug")
2. Claude Code spawns teammate processes
3. Teammates work independently, communicate via sendMessage
4. Team lead can send `shutdown_request`, teammates confirm with `shutdown_response`
5. Sessions terminate cleanly

---

## Design Overview

AnyClaude does NOT implement the Agent Teams protocol itself. Claude Code handles
all team coordination internally. AnyClaude's role is:

| Responsibility | Description |
|----------------|-------------|
| **Enable** | Inject `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` via settings menu |
| **Configure** | Pass `--teammate-mode` flag at startup |
| **Detect** | Watch `.claude/teams/` for active teams |
| **Display** | Team dashboard (Ctrl+T) with status and task overview |
| **Route** | Per-agent backend routing via proxy token mapping |
| **Render** | Split-pane TUI showing each agent's terminal (Phase 2c) |
| **Aggregate** | Combined metrics/cost across all agents |

---

## Phase 2a: Team Dashboard (Ctrl+T)

### Goal

Detect active Agent Teams and display a dashboard popup with team status,
teammate list, and task summary.

### User Experience

**No active team:**

```
Ctrl+T  →  ┌───────────────────────────────────────────┐
            │  Agent Teams                       [Ctrl+T] │
            │                                             │
            │  No active team detected.                   │
            │                                             │
            │  To start a team, enable Agent Teams in     │
            │  Settings (Ctrl+E) and ask Claude Code to   │
            │  create a team in your prompt.               │
            │                                             │
            │  [Close]                                    │
            └─────────────────────────────────────────────┘
```

**Active team:**

```
Ctrl+T  →  ┌───────────────────────────────────────────┐
            │  Agent Teams: "debug-session"       [Ctrl+T] │
            │                                             │
            │  ── Members ──                              │
            │                                             │
            │  ● Lead (you)                               │
            │    Backend: Anthropic (Opus)                 │
            │    Status: active                           │
            │                                             │
            │  ● Teammate 1: "investigator-a"             │
            │    Backend: OpenRouter (Sonnet)   [Ctrl+B]   │
            │    Status: working on task #2               │
            │                                             │
            │  ● Teammate 2: "investigator-b"             │
            │    Backend: OpenRouter (Sonnet)   [Ctrl+B]   │
            │    Status: idle                             │
            │                                             │
            │  ── Tasks ──                                │
            │                                             │
            │  [x] Check authentication flow              │
            │  [~] Analyze database performance  (#2)     │
            │  [ ] Review error logs                      │
            │                                             │
            │  2 pending · 1 in progress · 1 done         │
            │                                             │
            │  [View Panes]  [Stop Team]  [Close]         │
            └─────────────────────────────────────────────┘
```

### Team Detection: FileWatcher

Watch `~/.claude/teams/` directory for changes:

```rust
pub struct TeamWatcher {
    /// Path to ~/.claude/teams/
    teams_dir: PathBuf,
    /// Currently detected active team
    active_team: Option<TeamInfo>,
    /// Polling interval
    poll_interval: Duration,
}

pub struct TeamInfo {
    pub id: String,
    pub name: String,
    pub members: Vec<TeamMember>,
    pub tasks: Vec<TeamTask>,
    pub created_at: DateTime<Utc>,
}

pub struct TeamMember {
    pub id: String,
    pub name: String,
    pub role: TeamRole, // Lead or Teammate
    pub status: AgentStatus,
}

pub enum TeamRole {
    Lead,
    Teammate,
}

pub struct TeamTask {
    pub id: String,
    pub title: String,
    pub status: TaskStatus, // Pending, InProgress, Done
    pub owner: Option<String>,
    pub dependencies: Vec<String>,
}
```

**Polling approach** (not inotify/kqueue):
- Poll every 2 seconds for directory changes
- Parse `config.json` for team metadata
- Parse task JSON files for task list
- Emit `AppEvent::TeamStateChanged(Option<TeamInfo>)`

Rationale: polling is simpler, cross-platform, and the 2s latency is acceptable
for a dashboard display. File watchers (notify crate) add complexity and edge cases.

### Architecture Changes (Phase 2a)

#### New Files

| File | Purpose |
|------|---------|
| `src/teams/watcher.rs` | `TeamWatcher` — polls `.claude/teams/` for active teams |
| `src/teams/types.rs` | `TeamInfo`, `TeamMember`, `TeamTask` structs |
| `src/teams/parser.rs` | Parse team config.json and task JSON files |
| `src/ui/popups/teams.rs` | Team dashboard popup state, rendering |

#### Modified Files

| File | Change |
|------|--------|
| `src/ui/app.rs` | Add `PopupKind::AgentTeams`, team state field |
| `src/ui/input.rs` | Add `Ctrl+T` handler |
| `src/ui/render.rs` | Render team dashboard popup |
| `src/ui/runtime.rs` | Spawn `TeamWatcher`, handle `TeamStateChanged` events |

#### New AppEvent Variants

```rust
pub enum AppEvent {
    // ... existing variants ...
    TeamStateChanged(Option<TeamInfo>),
}
```

---

## Phase 2b: Multi-PTY Pool & Per-Agent Backend Routing

### Goal

Manage multiple PTY sessions (one per agent) and route each through a
different backend via the proxy.

### Multi-PTY Architecture

Currently AnyClaude manages one `PtySession`. For Agent Teams, we need N sessions.

```rust
/// Manages multiple PTY sessions for agent teams
pub struct PtyPool {
    /// The lead session (always index 0)
    lead: PtySessionEntry,
    /// Teammate sessions
    teammates: Vec<PtySessionEntry>,
    /// Currently focused session (receives keyboard input)
    focused: usize,
}

pub struct PtySessionEntry {
    pub id: String,                     // agent/teammate ID
    pub name: String,                   // display name
    pub session: PtySession,
    pub handle: PtyHandle,
    pub backend_id: Option<String>,     // per-agent backend override
    pub proxy_token: String,            // unique auth token for proxy routing
    pub status: AgentStatus,
}

pub enum AgentStatus {
    Starting,
    Active,
    Idle,
    Working { task_description: String },
    Shutdown,
}

impl PtyPool {
    /// Get the focused session's handle (for keyboard input routing)
    pub fn focused_handle(&self) -> &PtyHandle { ... }

    /// Switch focus to session by index
    pub fn set_focus(&mut self, index: usize) { ... }

    /// Add a new teammate session
    pub fn add_teammate(&mut self, entry: PtySessionEntry) { ... }

    /// Remove a teammate session (on shutdown)
    pub fn remove_teammate(&mut self, id: &str) { ... }

    /// Iterate all sessions (for rendering)
    pub fn iter(&self) -> impl Iterator<Item = &PtySessionEntry> { ... }
}
```

### Per-Agent Backend Routing

Each PTY session gets a unique auth token. The proxy maps token → backend.

```
Lead session:
  ANTHROPIC_AUTH_TOKEN = token-lead-uuid
  → Proxy routes to: Anthropic (Opus) backend

Teammate 1:
  ANTHROPIC_AUTH_TOKEN = token-mate1-uuid
  → Proxy routes to: OpenRouter (Sonnet) backend

Teammate 2:
  ANTHROPIC_AUTH_TOKEN = token-mate2-uuid
  → Proxy routes to: OpenRouter (Sonnet) backend
```

#### TokenBackendMap

New component in the proxy router:

```rust
/// Extended routing: auth token → backend mapping
pub struct TokenBackendMap {
    /// token → backend_id
    map: RwLock<HashMap<String, String>>,
    /// Fallback when token not found
    default_backend: String,
}

impl TokenBackendMap {
    /// Resolve backend for a given auth token
    pub fn resolve_backend(&self, token: &str) -> String {
        self.map.read()
            .get(token)
            .cloned()
            .unwrap_or_else(|| self.default_backend.clone())
    }

    /// Register token → backend mapping (on teammate spawn)
    pub fn register(&self, token: String, backend_id: String) {
        self.map.write().insert(token, backend_id);
    }

    /// Remove mapping (on teammate shutdown)
    pub fn unregister(&self, token: &str) {
        self.map.write().remove(token);
    }
}
```

#### Proxy Router Changes

Current flow: all requests validate against single session token, route to active backend.

New flow:
1. Extract `Authorization: Bearer <token>` from request
2. Look up token in `TokenBackendMap`
3. If found → route to mapped backend
4. If not found → reject (401) or fallback to default

```rust
// In proxy router handler:
let token = extract_bearer_token(&request);
let backend_id = token_backend_map.resolve_backend(&token);
let backend = config.get_backend(&backend_id);
// ... forward request to backend
```

### Cost Optimization Pattern

Primary use case: expensive lead + cheap teammates.

```toml
# Default backend for teammates
[claude_settings]
teammate_default_backend = "openrouter-sonnet"

# Lead uses the main active backend (Ctrl+B selection)
# Teammates auto-assigned to teammate_default_backend on spawn
```

Config in `~/.config/anyclaude/config.toml`:

```toml
[[backends]]
name = "anthropic"
display_name = "Anthropic (Lead)"
base_url = "https://api.anthropic.com"
auth_type = "passthrough"

[backends.pricing]
input_per_million = 15.00
output_per_million = 75.00

[[backends]]
name = "openrouter-sonnet"
display_name = "OpenRouter (Teammates)"
base_url = "https://openrouter.ai/api/v1"
auth_type = "bearer"
api_key = "${OPENROUTER_API_KEY}"
thinking_compat = true
thinking_budget_tokens = 10000

[backends.pricing]
input_per_million = 3.00
output_per_million = 15.00
```

### Architecture Changes (Phase 2b)

#### New Files

| File | Purpose |
|------|---------|
| `src/pty/pool.rs` | `PtyPool` — manages N PTY sessions |
| `src/proxy/token_map.rs` | `TokenBackendMap` — token → backend routing |

#### Modified Files

| File | Change |
|------|--------|
| `src/proxy/router.rs` | Integrate `TokenBackendMap` into request routing |
| `src/ui/runtime.rs` | Use `PtyPool` instead of single `PtySession` |
| `src/ui/app.rs` | Track focused pane, per-agent backend state |
| `src/ui/input.rs` | Route keyboard input to focused pane |
| `src/config/types.rs` | Add `teammate_default_backend` to config |

---

## Phase 2c: Split Pane TUI

### Goal

Render multiple agent terminals simultaneously in split panes within the TUI.

### Layout Engine

```rust
pub enum PaneLayout {
    /// Single pane (no team / team with 1 member)
    Single,
    /// Two panes side by side
    HorizontalSplit,
    /// 2x2 grid (3-4 agents)
    Grid2x2,
    /// 1 large + 2 small (leader prominent)
    LeaderFocus,
}

impl PaneLayout {
    /// Auto-select layout based on agent count
    pub fn for_agent_count(count: usize) -> Self {
        match count {
            0 | 1 => Self::Single,
            2 => Self::HorizontalSplit,
            3..=4 => Self::Grid2x2,
            _ => Self::LeaderFocus,
        }
    }

    /// Calculate ratatui Rect for each pane
    pub fn compute_rects(&self, area: Rect) -> Vec<PaneRect> { ... }
}

pub struct PaneRect {
    pub agent_index: usize,
    pub rect: Rect,
    pub is_focused: bool,
}
```

### Pane Rendering

Each pane renders:
1. **Border** — highlighted if focused, dim if unfocused
2. **Title bar** — agent name, backend name, status
3. **Terminal content** — from that agent's `PtyHandle` terminal emulator
4. **Status line** — current task (if any)

```
┌─ Lead (Anthropic/Opus) ● active ──────────┐
│                                            │
│  > Analyzing authentication flow...        │
│  Reading src/auth/middleware.rs             │
│                                            │
│  Task: Check auth flow                     │
└────────────────────────────────────────────┘
```

### Keyboard Routing

| Key | Context | Action |
|-----|---------|--------|
| Ctrl+1 / Ctrl+2 / Ctrl+3 / Ctrl+4 | Any | Focus pane by number |
| Ctrl+T | Any | Toggle team dashboard |
| Ctrl+B | Any | Switch backend for focused pane's agent |
| All other keys | Terminal focus | Route to focused pane's PtyHandle |

### Resize Handling

On terminal resize (`AppEvent::Resize`):
1. Recompute `PaneLayout` rects
2. Resize each `PtyHandle` to its new pane dimensions
3. Each pane's terminal emulator reflows content

### Architecture Changes (Phase 2c)

#### New Files

| File | Purpose |
|------|---------|
| `src/ui/layout/panes.rs` | `PaneLayout` engine, rect computation |
| `src/ui/components/pane.rs` | Single pane widget (border, title, terminal, status) |

#### Modified Files

| File | Change |
|------|--------|
| `src/ui/render.rs` | Multi-pane rendering when team active |
| `src/ui/input.rs` | Ctrl+1/2/3/4 focus switching, input routing to focused pane |
| `src/ui/app.rs` | Layout state, focused pane tracking |

---

## Metrics Aggregation

With multiple agents, the Status popup (Ctrl+S) should show aggregate data:

```
Ctrl+S  →  ┌───────────────────────────────────────────┐
            │  Status                             [Ctrl+S] │
            │                                             │
            │  ── Team Totals ──                          │
            │  Total requests: 47                         │
            │  Total tokens: 125,000 in / 43,000 out      │
            │  Estimated cost: $2.34                       │
            │                                             │
            │  ── Per Agent ──                            │
            │  Lead (Anthropic):     $1.80  (32 req)      │
            │  investigator-a (OR):  $0.32  (8 req)       │
            │  investigator-b (OR):  $0.22  (7 req)       │
            │                                             │
            │  ── Connection ──                           │
            │  Proxy: 127.0.0.1:8080 (active)             │
            │  Uptime: 1h 23m                             │
            └─────────────────────────────────────────────┘
```

---

## Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| Agent Teams API changes (experimental) | Breaking changes | Abstract team detection behind trait, minimize coupling to file format |
| Multi-PTY resource usage (5 agents) | High CPU/memory | Lazy PTY allocation, configurable max teammates, reduced scrollback per agent |
| Per-agent routing complexity | Proxy bugs | Extensive integration tests, fallback to default backend on unknown token |
| `--teammate-mode in-process` keybinding conflicts | Shift+Up/Down collide with AnyClaude | Passthrough all Shift+* to PTY when team active |
| Terminal emulator per agent (5x alacritty_terminal) | Memory (~50MB each) | Share scrollback config, limit per-agent `scrollback_lines = 1000` |
| Team file format undocumented | Parsing failures | Graceful degradation: show "team detected" without details if parsing fails |
| Teammate process spawning is managed by Claude Code, not us | Can't control teammate CLI flags | Only control lead's flags; teammates inherit Claude Code's internal defaults |

---

## Open Questions

1. **How to handle `--teammate-mode in-process` inside AnyClaude?**
   Claude Code's in-process mode uses Shift+Up/Down for teammate selection.
   These keybindings may conflict with AnyClaude's hotkeys.
   **Options:**
   - Passthrough all Shift+* when team is active
   - Override in-process navigation with our own pane focus (Ctrl+1/2/3)
   - Only support `--teammate-mode tmux` interception

2. **Per-agent backend assignment UI flow.**
   **Options:**
   - Auto-assign: all teammates get `teammate_default_backend` from config
   - Manual: user picks backend for each teammate in team dashboard
   - Hybrid: auto-assign with manual override via Ctrl+B on focused pane
   **Recommendation:** Hybrid approach. Config provides default, Ctrl+B overrides.

3. **Metrics aggregation granularity.**
   Should Ctrl+S show per-agent breakdown or only totals?
   **Recommendation:** Totals by default in Ctrl+S, per-agent in Ctrl+T dashboard.

4. **Should AnyClaude manage teammate lifecycle?**
   Currently Claude Code spawns/terminates teammates. Should AnyClaude add
   ability to manually spawn/stop individual teammates?
   **Recommendation:** No. Respect Claude Code's team management. AnyClaude only
   observes and provides display/routing. Adding lifecycle control couples us too
   tightly to internal Agent Teams implementation.

5. **tmux interception feasibility (Phase 2c).**
   Can AnyClaude pretend to be tmux so Claude Code's `--teammate-mode tmux`
   creates panes that we control?
   **Investigation needed:** What tmux API does Claude Code use? Is it `tmux split-window`
   commands? If so, we could intercept those commands in PATH with a shim script that
   notifies AnyClaude to create a new PTY pane instead.

---

## Future Possibilities

- **Cross-machine Agent Teams:** Teammates on remote machines, lead local. AnyClaude
  proxies API requests from remote teammates through local proxy for unified routing.
- **Team templates:** Pre-configured team compositions (e.g., "Debug Team", "Review Team")
  launchable from Ctrl+T menu.
- **Task board view:** Full-screen task management view (like a mini Kanban) showing
  all team tasks, owners, and dependencies.
- **Agent chat view:** Display inter-agent messages (from `.claude/teams/<id>/inbox/`)
  in a dedicated pane or popup for observability.
