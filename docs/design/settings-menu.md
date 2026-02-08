# Claude Code Settings Menu (Ctrl+E)

**Date**: 2026-02-08
**Status**: Draft / RFC

## Motivation

Claude Code has many startup-only flags, environment variables, and experimental features
that cannot be changed during a running session. Currently, changing any of them requires
the user to manually exit Claude Code, edit environment/config, and restart.

AnyClaude already manages the PTY lifecycle of Claude Code and injects env vars at startup.
We can leverage this to provide an in-app settings menu that modifies startup parameters
and transparently restarts Claude Code with `--continue`, preserving the session context.

## Goal

Provide a popup where users toggle Claude Code startup flags, experimental features,
and environment variables. On "Apply", AnyClaude gracefully restarts the Claude Code
process with new settings and `--continue` to preserve conversation context.

---

## User Experience

```
Ctrl+E  →  ┌─────────────────────────────────────────────┐
            │  Claude Code Settings                [Ctrl+E] │
            │                                               │
            │  ── Experimental Features ──                  │
            │  [ ] Agent Teams                              │
            │  [ ] Disable Auto Memory                      │
            │  [ ] Disable Background Tasks                 │
            │                                               │
            │  ── Startup Flags ──                          │
            │  Verbose:          [off]                      │
            │  Max turns:        [___]                      │
            │  Permission mode:  [default ▾]                │
            │  Teammate mode:    [auto ▾]                   │
            │  Effort level:     [high ▾]                   │
            │                                               │
            │  ── Tool Restrictions ──                      │
            │  Allowed tools:    [________________]         │
            │  Disallowed tools: [________________]         │
            │                                               │
            │  ── Provider ──                               │
            │  Use Bedrock:  [ ]                            │
            │  Use Vertex:   [ ]                            │
            │  Use Foundry:  [ ]                            │
            │                                               │
            │  ── Context ──                                │
            │  Max output tokens:  [32000]                  │
            │  Autocompact %:      [___]                    │
            │                                               │
            │  [Apply & Restart]  [Cancel]                  │
            │                                               │
            │  ⚠ Apply will restart Claude Code.            │
            │    Session continues via --continue.           │
            └───────────────────────────────────────────────┘
```

### Navigation

- Up/Down: navigate fields
- Space/Enter: toggle checkboxes, open dropdowns
- Tab: jump between sections
- Enter on "Apply & Restart": apply changes
- Esc / Ctrl+E: cancel and close

---

## Settings Data Model

```rust
/// Persisted to ~/.config/anyclaude/config.toml under [claude_settings]
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ClaudeSettings {
    // Experimental flags (env vars)
    pub agent_teams: bool,              // CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS
    pub disable_auto_memory: bool,      // CLAUDE_CODE_DISABLE_AUTO_MEMORY
    pub disable_background_tasks: bool, // CLAUDE_CODE_DISABLE_BACKGROUND_TASKS
    pub disable_telemetry: bool,        // DISABLE_TELEMETRY

    // Startup flags
    pub verbose: bool,                  // --verbose
    pub max_turns: Option<u32>,         // --max-turns
    pub permission_mode: Option<PermissionMode>, // --permission-mode
    pub teammate_mode: Option<TeammateMode>,     // --teammate-mode
    pub effort_level: Option<EffortLevel>,       // CLAUDE_CODE_EFFORT_LEVEL

    // Tool restrictions
    pub allowed_tools: Vec<String>,     // --allowedTools
    pub disallowed_tools: Vec<String>,  // --disallowedTools

    // Provider overrides
    pub use_bedrock: bool,              // CLAUDE_CODE_USE_BEDROCK
    pub use_vertex: bool,               // CLAUDE_CODE_USE_VERTEX
    pub use_foundry: bool,              // CLAUDE_CODE_USE_FOUNDRY

    // Context/token settings
    pub max_output_tokens: Option<u32>,       // CLAUDE_CODE_MAX_OUTPUT_TOKENS
    pub autocompact_pct: Option<u8>,          // CLAUDE_AUTOCOMPACT_PCT_OVERRIDE
    pub max_thinking_tokens: Option<u32>,     // MAX_THINKING_TOKENS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    Plan,
    BypassPermissions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TeammateMode {
    Auto,
    InProcess,
    Tmux,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EffortLevel {
    Low,
    Medium,
    High,
}
```

---

## Config Persistence

New section in `~/.config/anyclaude/config.toml`:

```toml
[claude_settings]
agent_teams = false
verbose = false
permission_mode = "default"
teammate_mode = "auto"
effort_level = "high"
max_turns = 0                # 0 = unlimited
allowed_tools = []
disallowed_tools = []
use_bedrock = false
use_vertex = false
use_foundry = false
max_output_tokens = 32000
autocompact_pct = 0          # 0 = default
max_thinking_tokens = 0      # 0 = default
```

Settings are saved to `config.toml` so they survive AnyClaude restarts.
Next launch uses the same Claude Code flags automatically.

---

## Restart Flow

```
User presses "Apply & Restart"
    │
    ├─ 1. Compare old vs new ClaudeSettings
    │     - If equal → close popup, no restart
    │
    ├─ 2. Persist ClaudeSettings to config.toml
    │
    ├─ 3. Build new env vars map from ClaudeSettings
    │     ┌────────────────────────────────────────────────┐
    │     │ ANTHROPIC_BASE_URL     = <proxy_base_url>      │ ← always
    │     │ ANTHROPIC_AUTH_TOKEN   = <session_token>        │ ← always
    │     │ CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS = "1"      │ ← if toggled
    │     │ CLAUDE_CODE_EFFORT_LEVEL = "high"               │ ← if set
    │     │ CLAUDE_CODE_MAX_OUTPUT_TOKENS = "32000"         │ ← if set
    │     │ ...                                             │
    │     └────────────────────────────────────────────────┘
    │
    ├─ 4. Build CLI args
    │     claude --continue [--verbose] [--max-turns N]
    │            [--permission-mode X] [--teammate-mode Y]
    │            [--allowedTools "A" "B"] [--disallowedTools "C"]
    │
    ├─ 5. Graceful PTY shutdown (existing logic from session.rs)
    │     - Close writer → SIGTERM → 300ms → SIGKILL
    │     - Join reader thread
    │
    ├─ 6. Spawn new PtySession with new env + args
    │     - Reuse existing proxy (no restart needed)
    │     - Same session token (proxy auth unchanged)
    │
    ├─ 7. Transition PtyLifecycleState: Ready → Pending → Attached → Ready
    │
    └─ 8. Close popup, return focus to terminal
```

### What `--continue` Restores

From official documentation:

- Full conversation history IS restored (all messages, tool uses, results)
- Session-scoped permissions are NOT restored (user re-approves)
- Session ID is reused (same conversation thread)
- `--continue --fork-session` creates a new session ID preserving history (branch-off)

---

## Key Design Decisions

**Proxy survives restart.** Only the PTY process restarts. The proxy keeps running
with the same bind address, session token, and active backend. This means:
- No interruption to proxy metrics/state
- Backend selection preserved across restart
- Session token unchanged (no re-auth needed)

**Diff-based restart.** Only restart if settings actually changed. Compare old vs
new `ClaudeSettings` via `PartialEq` — if equal, just close the popup.

**Env var cleanup.** When a setting is toggled OFF, the corresponding env var must
NOT be present in the new process. We build the env map from scratch each time
(not inheriting parent env blindly) to ensure clean state.

**Spinner during restart.** The terminal area shows a centered "Restarting Claude Code..."
message during the PTY restart gap (typically 1–3 seconds). Once the new PTY emits
output, the lifecycle transitions to Ready and normal rendering resumes.

---

## Settings → Env/Args Mapping

```rust
impl ClaudeSettings {
    /// Build environment variables for PTY spawn
    pub fn to_env_vars(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();

        if self.agent_teams {
            env.push(("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".into(), "1".into()));
        }
        if self.disable_auto_memory {
            env.push(("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into()));
        }
        if self.disable_background_tasks {
            env.push(("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".into(), "1".into()));
        }
        if self.disable_telemetry {
            env.push(("DISABLE_TELEMETRY".into(), "1".into()));
        }
        if let Some(ref level) = self.effort_level {
            env.push(("CLAUDE_CODE_EFFORT_LEVEL".into(), level.to_string()));
        }
        if let Some(tokens) = self.max_output_tokens {
            env.push(("CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(), tokens.to_string()));
        }
        if let Some(pct) = self.autocompact_pct {
            if pct > 0 {
                env.push(("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE".into(), pct.to_string()));
            }
        }
        if let Some(tokens) = self.max_thinking_tokens {
            env.push(("MAX_THINKING_TOKENS".into(), tokens.to_string()));
        }
        if self.use_bedrock {
            env.push(("CLAUDE_CODE_USE_BEDROCK".into(), "1".into()));
        }
        if self.use_vertex {
            env.push(("CLAUDE_CODE_USE_VERTEX".into(), "1".into()));
        }
        if self.use_foundry {
            env.push(("CLAUDE_CODE_USE_FOUNDRY".into(), "1".into()));
        }

        env
    }

    /// Build CLI arguments for Claude Code
    pub fn to_cli_args(&self) -> Vec<String> {
        let mut args = vec!["--continue".to_string()];

        if self.verbose {
            args.push("--verbose".to_string());
        }
        if let Some(turns) = self.max_turns {
            if turns > 0 {
                args.push("--max-turns".to_string());
                args.push(turns.to_string());
            }
        }
        if let Some(ref mode) = self.permission_mode {
            args.push("--permission-mode".to_string());
            args.push(mode.to_string());
        }
        if let Some(ref mode) = self.teammate_mode {
            args.push("--teammate-mode".to_string());
            args.push(mode.to_string());
        }
        for tool in &self.allowed_tools {
            args.push("--allowedTools".to_string());
            args.push(tool.clone());
        }
        for tool in &self.disallowed_tools {
            args.push("--disallowedTools".to_string());
            args.push(tool.clone());
        }

        args
    }
}
```

---

## Architecture Changes

### New Files

| File | Purpose |
|------|---------|
| `src/config/claude_settings.rs` | `ClaudeSettings` struct, serialization, diff, env/args mapping |
| `src/ui/popups/settings.rs` | Settings popup state, form widgets, rendering, input handling |

### Modified Files

| File | Change |
|------|--------|
| `src/config/types.rs` | Add `claude_settings: ClaudeSettings` to `AppConfig` |
| `src/ui/app.rs` | Add `PopupKind::Settings`, settings form state, restart request |
| `src/ui/input.rs` | Add `Ctrl+E` handler, settings popup navigation/editing |
| `src/ui/render.rs` | Render settings popup with form widgets |
| `src/ui/runtime.rs` | Handle `AppEvent::RestartClaude`, PTY shutdown + respawn |
| `src/pty/session.rs` | Extract env/args builder for reuse in restart path |

### New AppEvent Variant

```rust
pub enum AppEvent {
    // ... existing variants ...
    RestartClaude {
        env: Vec<(String, String)>,
        args: Vec<String>,
    },
}
```

### New UiCommand Variant

```rust
pub enum UiCommand {
    // ... existing variants ...
    RestartPty {
        settings: ClaudeSettings,
    },
}
```

---

## Popup UI Components

The settings popup requires new widget types beyond the current popup system:

| Widget | Description | Used For |
|--------|-------------|----------|
| **Checkbox** | `[x]` / `[ ]` toggle | Experimental flags, provider toggles |
| **Dropdown** | `[value ▾]` with popup list | Permission mode, teammate mode, effort |
| **NumberInput** | Editable numeric field | Max turns, tokens, autocompact % |
| **TextListInput** | Free-form text entry | Allowed/disallowed tools |
| **SectionHeader** | `── Title ──` divider | Visual grouping |

Implementation approach: build a generic `FormField` enum and a `FormState`
that tracks focused field index, edit mode, and field values.

```rust
pub enum FormField {
    Checkbox { label: String, value: bool },
    Dropdown { label: String, options: Vec<String>, selected: usize, open: bool },
    NumberInput { label: String, value: String, min: u32, max: u32 },
    TextListInput { label: String, items: Vec<String>, editing_index: Option<usize> },
    Section { title: String },
}

pub struct SettingsFormState {
    fields: Vec<FormField>,
    focused: usize,
    editing: bool,        // true when editing a text/number field
    scroll_offset: usize, // for long forms that exceed popup height
}
```

### Field Navigation Rules

- Up/Down: move `focused` index, skip `Section` headers
- Space: toggle `Checkbox`, open/close `Dropdown`
- Enter: confirm `Dropdown` selection, enter/exit edit mode for `NumberInput`/`TextListInput`
- Typing: when `editing == true`, modify the focused field's value
- Tab: jump to next section header (section-level navigation)

### Form ↔ ClaudeSettings Binding

The form is initialized from `ClaudeSettings` on popup open and written back
to `ClaudeSettings` on "Apply". A `form_to_settings()` / `settings_to_form()`
pair handles bidirectional conversion.

---

## Implementation Phases

### Phase 1a: PTY Restart Infrastructure

**Scope:** Enable graceful PTY restart with `--continue`, no UI yet.

- [ ] Add restart method to PTY management layer
- [ ] Handle `PtyLifecycleState` transition: Ready → Pending → Attached → Ready
- [ ] Show "Restarting..." overlay during transition
- [ ] Preserve proxy state across restart (no proxy restart)
- [ ] Add `RestartClaude` event to `AppEvent`
- [ ] Integration tests: restart preserves proxy, new env vars applied

### Phase 1b: Settings Data Model & Config

**Scope:** `ClaudeSettings` struct, persistence, env/args generation.

- [ ] Define `ClaudeSettings` in `src/config/claude_settings.rs`
- [ ] Add `[claude_settings]` to `AppConfig` and TOML parser
- [ ] Implement `to_env_vars()` and `to_cli_args()`
- [ ] Implement settings diff via `PartialEq` (only restart if changed)
- [ ] Load settings on startup, apply to initial PTY spawn
- [ ] Unit tests: serialization roundtrip, env mapping, args mapping, diff

### Phase 1c: Settings Popup UI

**Scope:** Ctrl+E popup with form widgets.

- [ ] Implement `FormField` widgets (checkbox, dropdown, number input, text list)
- [ ] Implement `SettingsFormState` with navigation and editing
- [ ] Add `PopupKind::Settings` to app state
- [ ] Add Ctrl+E handler to `input.rs`
- [ ] Render settings popup in `render.rs`
- [ ] Wire "Apply & Restart" to PTY restart flow
- [ ] Manual testing: toggle settings, verify restart, verify --continue works

---

## Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| `--continue` loses context after unclean exit | Session lost | Detect unclean exit, warn before restart |
| Restart takes too long (cold start) | UX stutter | Show spinner with elapsed time, timeout after 10s |
| Env var pollution from parent process | Wrong behavior | Build env from scratch, don't inherit blindly |
| Settings popup too tall for small terminals | UI overflow | Scrollable form with scroll indicator |
| Some flags incompatible with each other | Runtime errors | Validate combinations before restart |

---

## Open Questions

1. **Should settings also expose `settings.json` mutations?** (e.g., MCP server toggles)
   Some settings can be changed via `/config` within Claude Code, but toggling MCP servers
   requires file edit + restart. Could be a future addition.

2. **Should we support `--continue --fork-session`?** If the user wants to branch
   off before applying settings, we could offer a "Fork & Apply" button alongside
   "Apply & Restart". This creates a new session with the new settings while preserving
   the original session.

3. **Should settings popup show current Claude Code state?** (e.g., current model,
   current permission mode). This would require parsing Claude Code output or querying
   its state, which adds complexity.

---

## Reference: Startup-Only Settings (Complete List)

These are settings that CANNOT be changed during a running Claude Code session
and therefore benefit from the restart-with-`--continue` approach.

### Environment Variables

| Variable | Settings Menu Field | Type |
|----------|-------------------|------|
| `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` | Agent Teams | Checkbox |
| `CLAUDE_CODE_DISABLE_AUTO_MEMORY` | Disable Auto Memory | Checkbox |
| `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` | Disable Background Tasks | Checkbox |
| `DISABLE_TELEMETRY` | Disable Telemetry | Checkbox |
| `CLAUDE_CODE_EFFORT_LEVEL` | Effort Level | Dropdown (low/medium/high) |
| `CLAUDE_CODE_MAX_OUTPUT_TOKENS` | Max Output Tokens | Number (max 64000) |
| `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` | Autocompact % | Number (1–100) |
| `MAX_THINKING_TOKENS` | Max Thinking Tokens | Number |
| `CLAUDE_CODE_USE_BEDROCK` | Use Bedrock | Checkbox |
| `CLAUDE_CODE_USE_VERTEX` | Use Vertex | Checkbox |
| `CLAUDE_CODE_USE_FOUNDRY` | Use Foundry | Checkbox |
| `BASH_DEFAULT_TIMEOUT_MS` | Bash Timeout | Number |
| `BASH_MAX_OUTPUT_LENGTH` | Bash Max Output | Number |
| `CLAUDE_CODE_ENABLE_TASKS` | Enable Tasks | Checkbox |
| `DISABLE_COST_WARNINGS` | Disable Cost Warnings | Checkbox |

### CLI Flags

| Flag | Settings Menu Field | Type |
|------|-------------------|------|
| `--verbose` | Verbose | Checkbox |
| `--max-turns` | Max Turns | Number |
| `--permission-mode` | Permission Mode | Dropdown (default/acceptEdits/plan/bypassPermissions) |
| `--teammate-mode` | Teammate Mode | Dropdown (auto/in-process/tmux) |
| `--allowedTools` | Allowed Tools | Text list |
| `--disallowedTools` | Disallowed Tools | Text list |
| `--debug` | Debug Mode | Checkbox + text (categories) |
| `--chrome` / `--no-chrome` | Chrome Integration | Checkbox |
| `--disable-slash-commands` | Disable Slash Commands | Checkbox |

### Not Exposed (Advanced / Rare)

These exist but are too specialized for a general settings UI:

- `--system-prompt`, `--append-system-prompt` (SDK use cases)
- `--mcp-config`, `--strict-mcp-config` (managed separately)
- `--agent`, `--agents` (custom agent definitions)
- `--betas` (API internals)
- `--json-schema`, `--output-format` (SDK/print mode)
- `--max-budget-usd` (print mode only)
- `--add-dir` (changeable via `/add-dir` in session)
- Provider auth vars (managed via AnyClaude backend config)
