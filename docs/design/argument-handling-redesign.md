# Argument Handling System Redesign

## Status: IMPLEMENTED

## Problem Statement

The current argument handling in AnyClaude has grown organically and suffers from several structural issues:

1. **No single source of truth** — session flags (`--continue`, `--resume`, `--session-id`) are hardcoded inline in `spawn_config.rs:98`
2. **Two dead code paths** — `src/pty/command.rs` defines `parse_command()` using `std::env::args()` but it's never used; actual parsing happens in `main.rs` via clap + `PtySpawnConfig`
3. **Fragmented argument assembly** — final CLI args for `claude` are pieced together across 4 files:
   - `main.rs` — captures passthrough args via clap
   - `spawn_config.rs` — strips session flags, injects `--session-id`/`--resume`
   - `claude_settings.rs` — contributes flags from settings registry
   - `runtime.rs` — adds `--teammate-mode tmux` conditionally
4. **Hardcoded flag strings scattered** — `"--teammate-mode"` in `runtime.rs:101`, `"--session-id"` in `spawn_config.rs:77`, `"ANTHROPIC_BASE_URL"` in `spawn_config.rs:87`
5. **Brittle value detection** — `!next.starts_with("--")` at `spawn_config.rs:110` assumes all flags are long-form; breaks for `-v` style short flags
6. **No passthrough validation** — unknown flags silently forwarded to `claude`, typos go undetected
7. **Environment vars added ad-hoc** — some via `SettingDef.env_var`, some hardcoded in `spawn_config.rs`, some in `runtime.rs` shim logic

## Current Architecture (As-Is)

```
User CLI input
    │
    ▼
main.rs: Cli::parse() (clap)
    │  --backend → validated against config
    │  [args]... → Vec<String> passthrough
    │
    ▼
ui::run(backend_override, claude_args)
    │
    ▼
runtime.rs:
    │  config.defaults.active = backend_override
    │  teammate_cli_args = ["--teammate-mode", "tmux"] (if shim)
    │  shim_env = [(PATH, ...)] (if shim)
    │
    ▼
PtySpawnConfig::new("claude", claude_args, base_url)
    │  extract_session() strips --continue/--resume/--session-id
    │  determines session_id (user-provided or UUID)
    │
    ▼
spawn_config.build(extra_env, extra_args, SessionMode)
    │  base_args + --session-id/--resume + extra_args
    │  [("ANTHROPIC_BASE_URL", proxy_url)] + extra_env
    │
    ▼
PtySession::spawn(command, args, env, ...)
```

### Pain point map

| Problem | Location | Impact |
|---------|----------|--------|
| Session flags hardcoded | `spawn_config.rs:98` | Can't add new intercepted flags without editing core logic |
| `--teammate-mode` hardcoded | `runtime.rs:101` | Not in any registry, inconsistent with settings pattern |
| `ANTHROPIC_BASE_URL` always injected | `spawn_config.rs:87` | No way to opt out or override |
| Dead `command.rs` | `src/pty/command.rs` | Confusion about which parsing path is active |
| `starts_with("--")` check | `spawn_config.rs:110` | False positive on short flags like `-s` |
| Args assembled in 4 places | `main.rs`, `spawn_config.rs`, `claude_settings.rs`, `runtime.rs` | Hard to reason about final command line |

## Proposed Architecture (To-Be)

### Design Principles

1. **Single source of truth** — all flags (wrapper-owned + intercepted + passthrough) defined in one registry
2. **Declarative flag definitions** — describe flags with metadata, don't hand-roll parsing
3. **Explicit argument pipeline** — clear, traceable stages from user input to final `claude` invocation
4. **Environment and CLI unified** — env vars and CLI flags managed through the same system
5. **Testable in isolation** — each stage is a pure function that can be unit-tested

### Core Concept: `ArgPipeline`

Replace the current scattered logic with a structured pipeline:

```
User Input → Parse → Classify → Transform → Assemble → SpawnParams
```

Each stage has a clear contract and is independently testable.

### Stage 1: Flag Registry

A single, declarative registry of all flags the wrapper knows about.

```rust
/// src/args/registry.rs

/// How AnyClaude handles a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagBehavior {
    /// AnyClaude's own flag — consumed, never forwarded to claude.
    WrapperOwned,
    /// Intercepted from passthrough — consumed by wrapper, not forwarded.
    /// May be re-injected in modified form (e.g., --continue → --session-id).
    Intercepted,
    /// Known claude flag — forwarded as-is with optional validation.
    Passthrough,
}

/// Whether a flag takes a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagArity {
    /// Boolean flag, no value (e.g., --continue, --verbose).
    NoValue,
    /// Requires exactly one value (e.g., --session-id <ID>).
    RequiresValue,
    /// Optional value — present or absent (e.g., --model [NAME]).
    OptionalValue,
}

/// A single flag definition.
pub struct FlagDef {
    /// Primary long form (e.g., "--session-id").
    pub long: &'static str,
    /// Optional short form (e.g., "-s").
    pub short: Option<&'static str>,
    /// Does it take a value?
    pub arity: FlagArity,
    /// How the wrapper handles it.
    pub behavior: FlagBehavior,
    /// Human-readable description (for help text and warnings).
    pub description: &'static str,
}

/// Build the complete flag registry.
pub fn flag_registry() -> Vec<FlagDef> {
    vec![
        // === Wrapper-owned flags ===
        FlagDef {
            long: "--backend",
            short: Some("-b"),
            arity: FlagArity::RequiresValue,
            behavior: FlagBehavior::WrapperOwned,
            description: "Override default backend",
        },

        // === Intercepted flags (session management) ===
        FlagDef {
            long: "--session-id",
            short: None,
            arity: FlagArity::RequiresValue,
            behavior: FlagBehavior::Intercepted,
            description: "Use specific session ID",
        },
        FlagDef {
            long: "--resume",
            short: Some("-r"),
            arity: FlagArity::RequiresValue,
            behavior: FlagBehavior::Intercepted,
            description: "Resume a session by ID",
        },
        FlagDef {
            long: "--continue",
            short: Some("-c"),
            arity: FlagArity::NoValue,
            behavior: FlagBehavior::Intercepted,
            description: "Continue last session in current directory",
        },

        // === Known passthrough flags ===
        FlagDef {
            long: "--model",
            short: Some("-m"),
            arity: FlagArity::RequiresValue,
            behavior: FlagBehavior::Passthrough,
            description: "Model override",
        },
        FlagDef {
            long: "--verbose",
            short: Some("-v"),
            arity: FlagArity::NoValue,
            behavior: FlagBehavior::Passthrough,
            description: "Verbose output",
        },
        // ... more known claude flags as needed
    ]
}
```

**Benefits:**
- Adding a new intercepted flag = one `FlagDef` entry
- Short flags (`-r`, `-c`) supported naturally via `short` field
- `FlagArity` eliminates the `starts_with("--")` heuristic

### Stage 2: Argument Classifier

Replaces the hand-rolled iterator in `extract_session()`. Takes raw args + registry → classified args.

```rust
/// src/args/classifier.rs

/// A classified argument.
#[derive(Debug, Clone)]
pub enum ClassifiedArg {
    /// Wrapper-owned flag (already consumed by clap, shouldn't appear here).
    WrapperOwned { flag: String, value: Option<String> },
    /// Intercepted flag with optional value.
    Intercepted { flag: String, value: Option<String> },
    /// Known passthrough flag with optional value.
    KnownPassthrough { flag: String, value: Option<String> },
    /// Unknown flag — not in registry. Forwarded with optional warning.
    UnknownPassthrough(String),
    /// Positional argument (not a flag).
    Positional(String),
}

pub struct ClassifyResult {
    pub args: Vec<ClassifiedArg>,
    pub warnings: Vec<String>,
}

/// Classify raw args against the registry.
pub fn classify(raw_args: &[String], registry: &[FlagDef]) -> ClassifyResult {
    // For each arg:
    //   1. Look up in registry by long or short form
    //   2. If found, classify + consume value based on FlagArity
    //   3. If not found and starts with '-', classify as UnknownPassthrough
    //   4. Otherwise, classify as Positional
    todo!()
}
```

**Key improvement:** The `FlagArity` field tells the classifier whether to consume the next token as a value — no more `starts_with("--")` heuristic.

### Stage 3: Session Resolver

Replaces `extract_session()`. Takes classified args → session decision.

```rust
/// src/args/session.rs

pub struct SessionResolution {
    /// Determined session ID.
    pub session_id: String,
    /// How the session was resolved (for logging/debugging).
    pub source: SessionSource,
    /// Warnings (conflicts, missing values, etc.)
    pub warnings: Vec<String>,
}

pub enum SessionSource {
    /// User passed --session-id <id>.
    ExplicitId,
    /// User passed --resume <id>.
    ResumeId,
    /// User passed --continue, resolved from ~/.claude.json or sessions-index.
    ContinueLast,
    /// No session flags — generated new UUID.
    Generated,
}

/// Resolve session from classified args.
pub fn resolve_session(classified: &[ClassifiedArg]) -> SessionResolution {
    // Extract Intercepted args, determine session ID.
    // Same logic as current extract_session(), but operating on
    // strongly-typed ClassifiedArg instead of raw strings.
    todo!()
}
```

### Stage 4: Environment Builder

Consolidates all env var logic into one place. Currently spread across `spawn_config.rs`, `claude_settings.rs`, and `runtime.rs`.

```rust
/// src/args/env_builder.rs

pub struct EnvSet {
    vars: Vec<(String, String)>,
}

impl EnvSet {
    pub fn new() -> Self { Self { vars: Vec::new() } }

    /// Always-present: proxy URL.
    pub fn with_proxy_url(mut self, url: &str) -> Self {
        self.vars.push(("ANTHROPIC_BASE_URL".into(), url.into()));
        self
    }

    /// From settings (agent teams, etc.)
    pub fn with_settings(mut self, settings: &ClaudeSettingsManager) -> Self {
        self.vars.extend(settings.to_env_vars());
        self
    }

    /// From teammate shim (PATH override).
    pub fn with_shim(mut self, shim: Option<&TeammateShim>) -> Self {
        if let Some(s) = shim {
            self.vars.push(s.path_env());
        }
        self
    }

    pub fn build(self) -> Vec<(String, String)> {
        self.vars
    }
}
```

### Stage 5: Argument Assembler

Consolidates all CLI arg logic into one place. Currently spread across `spawn_config.rs`, `claude_settings.rs`, and `runtime.rs`.

```rust
/// src/args/assembler.rs

pub struct ArgAssembler {
    args: Vec<String>,
}

impl ArgAssembler {
    /// Start with passthrough args (filtered from classified args).
    pub fn from_passthrough(classified: &[ClassifiedArg]) -> Self {
        let args = classified.iter().filter_map(|a| match a {
            ClassifiedArg::KnownPassthrough { flag, value } => {
                let mut v = vec![flag.clone()];
                if let Some(val) = value { v.push(val.clone()); }
                Some(v)
            }
            ClassifiedArg::UnknownPassthrough(s) => Some(vec![s.clone()]),
            ClassifiedArg::Positional(s) => Some(vec![s.clone()]),
            _ => None, // Wrapper-owned and Intercepted are consumed
        }).flatten().collect();
        Self { args }
    }

    /// Inject session flag based on mode.
    pub fn with_session(mut self, session: &SessionResolution, mode: SessionMode) -> Self {
        match mode {
            SessionMode::Initial => {
                self.args.push("--session-id".into());
                self.args.push(session.session_id.clone());
            }
            SessionMode::Resume => {
                self.args.push("--resume".into());
                self.args.push(session.session_id.clone());
            }
        }
        self
    }

    /// From settings (cli flags from registry).
    pub fn with_settings(mut self, settings: &ClaudeSettingsManager) -> Self {
        self.args.extend(settings.to_cli_args());
        self
    }

    /// From teammate shim (--teammate-mode tmux).
    pub fn with_teammate_mode(mut self, shim: Option<&TeammateShim>) -> Self {
        if shim.is_some() {
            self.args.push("--teammate-mode".into());
            self.args.push("tmux".into());
        }
        self
    }

    pub fn build(self) -> Vec<String> {
        self.args
    }
}
```

### Full Pipeline

```rust
/// src/args/pipeline.rs

pub struct SpawnParams {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub session_id: String,
    pub warnings: Vec<String>,
}

pub fn build_spawn_params(
    raw_args: &[String],
    mode: SessionMode,
    proxy_url: &str,
    settings: &ClaudeSettingsManager,
    shim: Option<&TeammateShim>,
) -> SpawnParams {
    let registry = flag_registry();

    // Stage 1: Classify
    let classified = classify(raw_args, &registry);

    // Stage 2: Resolve session
    let session = resolve_session(&classified.args);

    // Stage 3: Build env
    let env = EnvSet::new()
        .with_proxy_url(proxy_url)
        .with_settings(settings)
        .with_shim(shim)
        .build();

    // Stage 4: Assemble args
    let args = ArgAssembler::from_passthrough(&classified.args)
        .with_session(&session, mode)
        .with_settings(settings)
        .with_teammate_mode(shim)
        .build();

    // Collect all warnings
    let mut warnings = classified.warnings;
    warnings.extend(session.warnings);

    SpawnParams {
        command: "claude".into(),
        args,
        env,
        session_id: session.session_id,
        warnings,
    }
}
```

### New Module Structure

```
src/args/
├── mod.rs              # Public API: build_spawn_params(), SpawnParams
├── registry.rs         # FlagDef, FlagBehavior, FlagArity, flag_registry()
├── classifier.rs       # classify() — raw args → ClassifiedArg
├── session.rs          # resolve_session() — session ID determination
├── env_builder.rs      # EnvSet builder — all env vars in one place
├── assembler.rs        # ArgAssembler — all CLI args in one place
└── pipeline.rs         # build_spawn_params() — ties it all together
```

### What Gets Deleted

| File/Code | Reason |
|-----------|--------|
| `src/pty/command.rs` | Dead code. Unused `parse_command()` |
| `spawn_config.rs::extract_session()` | Replaced by `classifier.rs` + `session.rs` |
| `spawn_config.rs::build()` env/args logic | Replaced by `env_builder.rs` + `assembler.rs` |
| `runtime.rs:100-104` teammate args | Moved to `assembler.rs::with_teammate_mode()` |
| `runtime.rs:181-185` env/args assembly | Replaced by single `build_spawn_params()` call |

### What Changes

| File | Change |
|------|--------|
| `main.rs` | Cli struct stays (clap handles `--backend`). Pass `cli.args` to pipeline |
| `runtime.rs` | Replace 15 lines of scattered assembly with one `build_spawn_params()` call |
| `spawn_config.rs` | Simplifies to just hold `SpawnParams` + `session_id`, delegates to `args` module |
| `claude_settings.rs` | `to_cli_args()` and `to_env_vars()` stay, consumed by pipeline |

### Simplified `runtime.rs` (After)

```rust
// Before: 15+ lines across multiple locations
// After: one call
let spawn = args::build_spawn_params(
    &claude_args,
    SessionMode::Initial,
    &actual_base_url,
    app.settings_manager(),
    _teammate_shim.as_ref(),
);

for warning in &spawn.warnings {
    app.error_registry().record(ErrorSeverity::Warning, ErrorCategory::Process, warning);
}

let mut pty_session = PtySession::spawn(
    spawn.command,
    spawn.args,
    spawn.env,
    scrollback_lines,
    events.sender(),
    app.pty_generation(),
)?;
```

## Testing Strategy

Each stage is independently testable with pure functions:

```rust
#[cfg(test)]
mod tests {
    // Classifier tests
    #[test]
    fn classify_known_long_flag_with_value() {
        let args = vec!["--session-id".into(), "abc123".into(), "--verbose".into()];
        let result = classify(&args, &flag_registry());
        assert!(matches!(result.args[0], ClassifiedArg::Intercepted { .. }));
        assert!(matches!(result.args[1], ClassifiedArg::KnownPassthrough { .. }));
    }

    #[test]
    fn classify_short_flag() {
        let args = vec!["-r".into(), "abc123".into()];
        let result = classify(&args, &flag_registry());
        assert!(matches!(result.args[0], ClassifiedArg::Intercepted { flag, .. } if flag == "--resume"));
    }

    #[test]
    fn classify_unknown_flag_warns() {
        let args = vec!["--typo-flag".into()];
        let result = classify(&args, &flag_registry());
        assert!(matches!(result.args[0], ClassifiedArg::UnknownPassthrough(_)));
        // Optionally: assert warning about unknown flag
    }

    // Session resolver tests
    #[test]
    fn session_continue_with_no_history() { ... }

    #[test]
    fn session_conflict_resume_and_continue() { ... }

    // Pipeline integration test
    #[test]
    fn full_pipeline_initial_spawn() {
        let params = build_spawn_params(
            &["--continue".into(), "--verbose".into()],
            SessionMode::Initial,
            "http://127.0.0.1:8080",
            &ClaudeSettingsManager::new(),
            None,
        );
        assert!(params.args.contains(&"--session-id".to_string()));
        assert!(params.args.contains(&"--verbose".to_string()));
        assert!(!params.args.contains(&"--continue".to_string())); // consumed
        assert!(params.env.iter().any(|(k, _)| k == "ANTHROPIC_BASE_URL"));
    }
}
```

## Migration Path

### Phase 1: Create `src/args/` module (non-breaking)
- Implement `registry.rs`, `classifier.rs`, `session.rs`
- Port `extract_session()` tests to new module
- Both old and new paths coexist

### Phase 2: Wire pipeline into `runtime.rs`
- Replace scattered assembly with `build_spawn_params()`
- `PtySpawnConfig` simplified to thin wrapper over `SpawnParams`
- Run existing integration tests to verify

### Phase 3: Cleanup
- Delete `src/pty/command.rs`
- Remove `extract_session()` from `spawn_config.rs`
- Remove inline arg assembly from `runtime.rs`

## Open Questions

1. **Unknown flag policy** — warn on unknown flags or silently forward? Current behavior is silent forward. Recommendation: log a debug warning, forward anyway.
2. **`--help` passthrough** — should `anyclaude --help` show combined help (wrapper + claude)? Or should `anyclaude -- --help` pass to claude?
3. **Restart args** — when PTY restarts (settings change), should the session resume automatically pick up new settings args? Current behavior: yes, via `spawn_config.build()` with new `extra_args`.
4. **Registry extensibility** — should the flag registry be compile-time only, or loadable from config? Recommendation: compile-time, with a `Passthrough` catch-all for unknown flags.
