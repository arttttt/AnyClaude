# Changelog

All notable changes to AnyClaude are documented in this file.

## [0.4.0] - 2026-02-15

### Bug Fixes

- Generate realistic SSE format in MockResponse::sse()
- Align body content with header/footer by adding 1-column side padding
- Forward Ctrl+V to CC instead of intercepting clipboard images
- Respect debug logging config in tmux shim
- Propagate routing decision to upstream forwarding
- Detect Shift+Enter via macOS CGEvent for newline insertion

### Documentation

- Add design document for reverse model mapping
- Add commit rules to AGENTS.md
- Add argument handling redesign documentation
- Update shim doc comment to reference args pipeline
- Update README with agent teams, model mapping, and CLI options
- Design thinking pipeline isolation for multi-agent sessions
- Update design doc with Phase 1b completion and model map
- Add Phase 1b/1c design for smart and synthetic tmux shims
- Update routing design with empirical tmux findings
- Revise routing design — generic routing layer + simple config
- Add per-agent backend routing design for Agent Teams

### Features

- Add double-click word selection
- Wire reverse model mapping into upstream proxy pipeline
- Add model_rewrite module for reverse model mapping
- Add ChunkRewriter to ObservedStream for response transformation
- Add args pipeline module for declarative argument handling
- Separate main and teammate pipelines via axum nest
- Add per-backend model family mapping (model_opus/sonnet/haiku)
- Smart tmux shim injects ANTHROPIC_BASE_URL for teammates
- Wire shims and --teammate-mode tmux into PTY spawn
- Add PATH shims for teammate routing (claude + tmux)
- Add proxy routing layer for path-based backend selection
- Add AgentTeamsConfig with teammate_backend validation

### Refactor

- Remove dead code replaced by args pipeline
- Integrate args pipeline into runtime
- Remove dead image paste code
- Remove redundant claude PATH shim
- Remove debug logging env var overrides
- Wire new pipeline architecture into server and IPC
- Extract ThinkingSession from request extensions in upstream
- Introduce ThinkingSession as per-request handle
- Remove dead code from thinking pipeline
- Make encode_project_path pub, restore tests

### Testing

- Add 32 tests for reverse model mapping
- Add integration tests for args pipeline
- Add pipeline isolation and ThinkingSession tests

## [0.3.1] - 2026-02-11

### Bug Fixes

- Detect Option key via macOS CGEvent for Warp terminal
- Propagate arrow keys with Control and Alt/Option modifiers
- Improve session flag handling, warnings, and clippy compliance
- Apply saved settings env vars on initial PTY spawn
- Use narrow centered scrollbar char with distinct color
- Add gap between content and scrollbar
- Move scrollbar inside dialog border
- Use █ for scrollbar thumb to contrast with border
- Replace ratatui Scrollbar with manual draw for constant thumb size
- Scrollbar thumb not reaching bottom of track
- Scrollbar not reaching bottom with small scroll range
- History dialog time alignment with multi-byte chars

### Chore

- Release v0.3.1
- Enforce no inline #[cfg(test)] in src/ via lint
- Remove .DS_Store from tracking

### Documentation

- Add design docs for settings menu and agent teams integration
- Consolidate test convention to single tests/ directory
- Add testing rules to AGENTS.md
- Add missing config fields to README Full Example
- Remove Ctrl+V from hotkeys table

### Features

- Add term_input crate for lossless raw byte terminal input
- Add mouse text selection and input improvements
- Save pasted images to temp files instead of data URIs
- Replace --continue with --session-id/--resume UUID targeting
- Add Settings Menu (Ctrl+E) with PTY restart
- Add settings configuration layer and PtySpawnConfig
- Buffer user input during PTY startup and flush on ready
- Migrate terminal emulator from vt100 to alacritty_terminal
- Always show Esc/Ctrl+H in history dialog footer
- Unified centered footer in PopupDialog, fix scrollbar
- Add legend and scrollbar to history dialog

### Refactor

- Replace crossterm event parsing with term_input
- Centralize PTY lifecycle with Restarting state and generation counter
- Unify logging by removing tracing in favor of DebugLogger
- Abstract terminal emulator behind TerminalEmulator trait
- Move scrollbar into PopupDialog component
- Extract unified PopupDialog into ui/components

### Testing

- Migrate all inline tests from src/ to tests/
- Add PTY lifecycle and startup readiness tests

## [0.3.0] - 2026-02-07

### Bug Fixes

- Resolve all clippy warnings, remove dead code, add workspace lints
- Prevent haiku sub-requests from evicting confirmed thinking blocks
- Resolve 7 concurrency issues from analysis
- Skip reqwest timeout for SSE streaming requests
- Structural SSE thinking event detection, replace naive text search
- Shared SSE parser and thinking cache eviction
- Require explicit thinking_compat=true, no auto-detect
- Fail fast on invalid config instead of silent fallback to defaults
- Respect thinking_budget_tokens config over max_tokens
- Serialize body after adaptive thinking conversion
- Log thinking compat events to debug.log via DebugLogger
- Properly accumulate SSE thinking deltas before registering blocks
- Call on_backend_switch for all modes and remove outdated comment
- Add error logging for filter serialization failure

### Chore

- Release v0.3.0
- Add justfile with release and check commands
- Add git-cliff config and generate CHANGELOG
- Add .DS_Store to gitignore

### Documentation

- Fix README config errors, add installation and development sections
- Add verification instructions to AGENTS.md
- Update README to reflect current architecture
- Document side-effect pattern in cancel/complete_summarization
- Add terminal emulator crate comparison analysis
- Add thinking blocks architecture documentation
- Add research findings to tool-context-preservation design

### Features

- Add backend history dialog (Ctrl+H)
- Convert adaptive thinking to enabled for non-Anthropic backends
- **ui:** Display thinking mode in header and status popup
- **thinking:** Implement NativeTransformer for passthrough mode
- **thinking:** Improve hash reliability with prefix + suffix
- **thinking:** Add confirmed flag and timestamp-based cleanup
- **thinking:** Add ThinkingRegistry for session-based thinking block tracking
- **debug:** Improve debug logging with full body capture and SSE summaries

### Refactor

- Remove dead code, fix bug, delete outdated docs post-cleanup
- Remove summarize and strip thinking modes, keep only native
- Remove dead SummarizeIntent::Success variant
- Add dispatch_mvi! macro and comprehensive MVI tests
- Unify history dialog visibility with focus management
- Eliminate RetrySummarization from InputAction, consolidate retry logic
- Embed button selection into SummarizeDialogState::Failed
- Remove dead MVI code (popup.rs, CancelSummarization, dead intents, Success state)
- Migrate PtyState to full MVI pattern
- Remove legacy retry-on-400 thinking block handling

### Testing

- Add unit tests for thinking compat functions

## [0.2.0] - 2026-02-03

### Bug Fixes

- Only save messages from chat completion requests
- **pty:** Buffer stdin input during Claude Code startup
- **config:** Remove ConfigWatcher to fix backend override race condition
- **ui:** Prevent rendering artifacts in terminal display
- **logging:** Disable logging by default in TUI mode
- **proxy:** Resolve 400 errors when switching backends
- **auth:** Replace AuthType::None with Passthrough for OAuth support
- **clipboard:** Inline image paste data URIs
- **clipboard:** Handle Ctrl+V for image paste
- **ui:** Header bar style matches footer bar formatting
- **pty:** Enable clipboard shortcuts passthrough (Ctrl+C/Ctrl+V)
- **ui:** Header bar style matches footer bar formatting
- **ui:** Remove tracing that corrupts TUI header display
- **ui:** Add arrow indicator for backend selector keyboard navigation
- **ui:** Apply highlight to spans for keyboard navigation
- **ui:** Improve backend selector popup layout and visibility
- Improve backend selector popup
- **config:** Restore ~/.config path fix that was lost in merge
- **proxy:** Strip auth headers before forwarding to upstream
- **config:** Use ~/.config path on all Unix platforms
- **ipc:** Add Display/Error traits, trace logging, and timeout test
- **metrics:** Improve timeout tracking and percentile calculation
- Consolidate upstream request timeout
- Address code review feedback
- **ui:** Polish spacing and clear scrollback
- **ui:** Add header borders
- **ui:** Size PTY to body
- **deps:** Restore portable-pty, crossterm, io-util; organize deps
- Inherit cwd for spawned claude

### Chore

- ClaudeWrapper → AnyClaude
- **deps:** Update major versions - thiserror, dirs, notify
- **deps:** Update crossterm 0.27 → 0.29, ratatui 0.26 → 0.30
- **deps:** Update portable-pty, axum, reqwest
- **deps:** Update toml, tower, signal-hook
- **deps:** Update uuid 1.12 → 1.20
- Drop refactor plan doc

### Documentation

- Remove hardcoded vendor references from architecture doc
- Fix motivation - focus on Anthropic-compatible backends
- Add goal section explaining the motivation
- Update README with current project state
- Simplify AGENTS.md to reference ARCHITECTURE.md
- Align observability design with implementation
- Remove temporary design doc
- Refresh agent instructions
- Add ARCHITECTURE.md and update AGENTS.md

### Features

- **Debug Mode & Request Logging:** Final implementation
- **proxy:** Add dynamic port allocation with fallback
- Add --backend CLI argument
- **terminal:** Migrate from termwiz to vt100 for scrollback support
- **mouse:** Implement proper mouse event forwarding to Claude Code
- **error:** Add centralized error registry and UI display
- **pty:** Auto-shutdown when Claude process terminates
- **shutdown:** Add graceful shutdown handling
- **clipboard:** Add image and file paste support
- **ui:** Implement network diagnostics popup (Ctrl+S)
- **ui:** Add backend selector popup behavior
- **ui:** Center popups by content size
- **thinking:** Add convert_to_tags mode for thinking blocks
- **Add convert_to_tags mode for thinking blocks:** Final implementation
- **config:** Remove models field from backend config
- **config:** Drop auth_env_var in favor of api_key
- **config:** Support direct api_key fallback
- **Remove models field from backend config:** Final implementation
- **config:** Add api_key field to Backend for direct key storage
- **Wire all components together:** Final implementation
- **proxy:** Add session auth and env injection
- **IPC layer for TUI communication:** Final implementation
- **metrics:** Add observability pipeline
- **proxy:** Implement connection pooling and retry with exponential backoff
- **backend:** Implement hot-swap routing for backend switching
- Implement error handling and timeouts
- **Config integration for upstream:** Final implementation
- Add SSE streaming support to proxy
- Add structured logging with tracing
- Implement graceful shutdown handling
- **config:** Add hot-reload with file watching and debouncing
- **config:** Add environment variable resolution for API keys
- **config:** Implement TOML config loader with validation
- **config:** Define Config, Defaults, and Backend structs with serde
- Route keyboard hotkeys
- **ui:** Render pty output
- Add hotkey footer hints
- **ui:** Render status header bar
- Compute body layout rect
- **ui:** Add color theme palette
- **ui:** Scaffold ratatui app runtime
- Handle PTY resize events
- **pty:** Restore PTY runner in module architecture
- Add termwiz vt parser wrapper
- Scaffold module layout
- **Implement input passthrough to PTY:** Final implementation
- **Proxy command-line arguments to claude process:** Final implementation
- **pty:** Enable interactive claude sessions
- **Initialize Rust project with core dependencies:** Final implementation

### Refactor

- Split proxy, metrics, and ipc modules for maintainability
- **config:** Remove unused models field from Backend
- **proxy:** Remove session auth, add passthrough mode
- Migrate proxy to axum and reqwest
- **ui,pty:** Split monolithic modules

### Testing

- Add comprehensive e2e testing suite
- Add CLI argument tests
- **thinking:** Add tests for backend switch scenarios
- Remove useless test_connection_tracking
- Add PTY passthrough integration tests


