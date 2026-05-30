# Claude Wrapper — Architecture

GPU-rendered terminal wrapper for Claude Code with hot-swappable backend support.

## Concept

Embed Claude Code in an in-process GPU terminal (PTY child), and run a local
reverse proxy that Claude Code talks to. The proxy routes requests to a chosen
backend; the user can hot-swap backends on the fly without restarting Claude
Code. The UI (header / footer chrome + popups) renders on top of the terminal.

## Principles

- **SOLID** — single responsibility, depend on existing seams, no leaky abstractions.
- **DRY** — extract a helper when the duplication is real.
- **KISS** — the simpler shape wins.

(YAGNI is *not* a project principle: deliberately forward-built engine
capability in `term_ui` / `uikit` is intentional, not cruft.)

## Entry point

`main.rs` → `anyclaude::ui::gpu::run` (`src/ui/gpu/bootstrap.rs`). `run` wires the
config / settings / debug logger / tokio runtime / proxy server / teammate shim,
prepares the Claude Code spawn params, builds the winit event loop, and hands off
to `GpuApp`. There is **no** `--gpu` flag and no legacy ratatui path — the GPU
stack is the only UI (the ratatui / crossterm / alacritty / arboard stack and the
old `src/{pty,ipc,ui/*.rs}` were removed in the cutover).

## Two subsystems

### A. Proxy / backend core (`src/`)

The local HTTP reverse proxy Claude Code is pointed at, plus backend state.

- **`proxy/`** — the reverse proxy: `server.rs` (bind + serve), `router.rs`
  (request routing + hot-swap), `pipeline/` (per-request stages: extract /
  resolve / model-rewrite / header / forward), `thinking/` (thinking-block
  registry + session cache), `sse.rs` + `src/sse.rs` (SSE parsing), `pool.rs`,
  `timeout.rs`, `connection.rs`, `health.rs`, `hooks.rs`, `model_rewrite.rs`,
  `shutdown.rs`.
- **`backend/`** — `BackendState` (active backend, switch log, validation) and
  the subagent / teammate `AgentBackendState` overrides.
- **`config/`** — config + Claude settings load/save (TOML), `ClaudeSettingsManager`.
- **`metrics/`** — `ObservabilityHub` (per-backend request counts / latency) +
  the per-session debug logger.
- **`args/`** — Claude Code spawn-param assembly (URL / session token / hooks).
- **`shim/`** — the teammate-routing shim prepended to `PATH`.

### B. GPU terminal UI (`src/ui/` + workspace crates)

A retained + reactive UI engine (replaced the old MVI / ratatui stack).

- **`ui/gpu/app/`** — the `GpuApp` coordinator (a winit `ApplicationHandler`),
  decomposed into `mod.rs` (struct + `new` + consts) plus responsibility
  submodules: `events` (the event loop + `ApplicationHandler`), `render` (the
  per-frame paint), `geometry` (cell metrics / grid fit / scroll bounds / mouse
  hit-test), `popups`, `clipboard`, `session_ops` (drain PTY / restart).
- **`ui/gpu/`** collaborators: `bootstrap` (entry), `overlay` (the chrome +
  popup `term_ui` retained trees and their paint pipeline), `session` (PTY child
  + VT emulator + spawn params), `text` (font system + caches + palette),
  `timers` (momentum / gesture-end / 1 Hz heartbeat), `backends` (proxy + config
  handles), `chrome` (dimension constants), `diagnostic` (debug-only state dump),
  `pty` (`ChildPty`).
- **`ui/app_state.rs`** — the single authoritative `AppState` (UI-decision state)
  plus the `Msg` / `Effect` / `apply` loop (see below).
- **`ui/`** pure presenters + state machines: `input` (key/shortcut mapping),
  `term_geometry` (layout/hit-test math), `chrome_labels` + `popup_view` (build
  the term_ui views), `popup_anim` (fade epoch), and the three popup state
  machines `backend_switch/` · `history/` · `settings/` (plain `apply()` enums,
  not MVI).

## The UI event loop (R10 / Elm-shaped)

Every winit / user event funnels through one cycle:

```
event → GpuApp::dispatch → AppState::apply(Msg, &ApplyCtx) -> Vec<Effect>
      → GpuApp::perform_effects(effects)
```

- **`AppState`** is plain owned data — no `Rc`/`RefCell`/GPU handles. It is the
  single state-*transition* point and is unit-testable without a window.
- **`apply`** is pure on `AppState` + a read-only `ApplyCtx`; every side effect
  (PTY write, clipboard, timers, redraw, popup toggle, …) comes back as an
  `Effect` for `perform_effects` — the one place state touches a resource.
- The coordinator pre-resolves resource-backed facts (the cell under the cursor,
  DECCKM / mouse-protocol state, the un-composed key) into a pure `Msg`, leaning
  on pure tested helpers in `term_gpu` (`encode_key` / `encode_mouse_report` /
  `encode_motion_report`). See the doc comment on `AppState::apply` for the
  reduce/resolve boundary.
- **Rendering (R5):** the chrome bars and popups are `term_ui` *retained* trees
  painted in the overlay; the terminal **grid** is a direct `populate_panel`
  full-emit each frame (the emulator has no dirty API, so a retained grid would
  re-emit wholesale anyway).

## Workspace crates (`crates/`)

anyclaude is the top crate; it depends on six lower crates (never the reverse):

- **`term_core`** — minimal VT parser + emulator (grid, scrollback, cursor,
  protocol flags). Not a full VT — scoped to Claude Code's ink TUI.
- **`term_gpu`** — wgpu renderer: glyph atlas (RGBA texture array, emoji-capable),
  cosmic-text shaping, `populate_panel`, cursor / selection rects, scroll math,
  key / mouse encoders.
- **`term_ui`** — the retained + reactive UI engine (arena tree, reconcile,
  flex-lite layout, paint). See `docs/design/term-ui-design.md` for the binding
  invariants R1–R15.
- **`uikit`** — reusable widgets over `term_ui` (chrome bars, popup list).
- **`term_clipboard`** — macOS `NSPasteboard` clipboard (text / HTML / paths /
  images), with an in-memory fallback off macOS.
- **`term_layout`** — BSP split-panel tree. Currently exercised only by
  `term_gpu` examples; the live app is single-panel.

## Tests

All tests live in `tests/` (anyclaude) and `crates/*/tests/`. No `#[cfg(test)]`
in `src/`. Workspace lints: `dead_code = "deny"`, `unused_imports = "deny"`
(`Cargo.toml`), so unused internal code can't compile. `cargo check` after each
commit; full `cargo test --workspace` at milestones.

## Dependency direction

```
main.rs → ui::gpu::run (bootstrap)
            ├── proxy / backend / config / metrics / args / shim   (the proxy core)
            └── GpuApp (ui/gpu/app)
                  ├── AppState  (ui/app_state — Msg/apply/Effect)
                  ├── term_ui / uikit         (retained chrome + popups)
                  ├── term_gpu / term_core    (grid render + VT)
                  └── term_clipboard
```

anyclaude → the `term_*` / `uikit` crates, never the reverse. For the UI engine's
detailed design and invariants, see `docs/design/term-ui-design.md`.
