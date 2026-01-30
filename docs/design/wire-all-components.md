# Wire All Components Together — Design Document

**Issue**: cl-tec.1
**Author**: polecat/obsidian
**Date**: 2026-01-31

## Overview

This document defines how the UI runtime, PTY session, proxy server, IPC layer,
config hot-reload, backend state, and metrics are wired into a single process
with clean lifecycle management and minimal cross-thread coupling.

## Goals

- Start proxy + IPC alongside the TUI and PTY.
- Route Claude Code traffic through the proxy via env injection.
- Allow UI to switch backends and read status/metrics via IPC.
- Propagate config reloads into proxy/backend state.
- Provide graceful shutdown across all components.

## Non-Goals

- UI design changes (layout, typography, or visuals).
- IPC feature expansion beyond existing commands.
- Cross-process IPC (everything remains in-process).

## Current State

- UI runtime is synchronous and uses `std::sync::mpsc` events.
- PTY session is spawned with env injection (proxy base URL + token).
- Proxy server, IPC, backend state, and metrics exist but are not started by the UI.
- Config hot-reload updates `ConfigStore` but does not update `BackendState`.

## Architecture

### Threads and Tasks

- **Main thread**: TUI render loop, input handling, and PTY I/O.
- **Config watcher thread**: reloads config and sends UI events.
- **Tokio runtime thread**: runs proxy server + IPC loop + async UI bridge.

### Data Flow

```
TUI (sync) ───────────────┐
  │ UI events            │
  │                      ▼
  │           UI↔Async bridge (tokio mpsc)
  │                      │
  │                      ▼
  │                 IPC client
  │                      │  (tokio mpsc + oneshot)
  │                      ▼
  │                 IPC server (proxy task)
  │                      │
  │                      ▼
  │       BackendState + ObservabilityHub
  │                      │
  └──── Claude Code (PTY) → Proxy Router → UpstreamClient → Backend
```

### Component Responsibilities

- **UI runtime**: owns `App`, handles input, draws TUI, hosts PTY.
- **Proxy server**: handles HTTP requests, auth token validation, routing.
- **IPC layer**: provides backend switching, status, metrics, backend list.
- **BackendState**: thread-safe, mutable active backend state.
- **ObservabilityHub**: request metrics + ring buffer.
- **ConfigStore + ConfigWatcher**: hot reload and snapshot reads.

## Startup Sequence

1. Load `Config` and create `ConfigStore`.
2. Generate session token (UUID v4).
3. Start Tokio runtime (background thread).
4. Inside Tokio runtime:
   - Create IPC layer (client + server).
   - Create ProxyServer with `ConfigStore` + session token.
   - Spawn proxy server task.
   - Spawn IPC server loop task (uses BackendState + ObservabilityHub owned by proxy).
   - Spawn UI↔async bridge task for IPC calls triggered by UI actions.
5. Spawn PTY with env injection using `proxy.base_url` + session token.
6. Start config watcher (debounced) to emit `AppEvent::ConfigReload`.
7. Enter TUI render loop.

## UI↔Async Bridge

The UI is synchronous, while IPC calls are async. A bridge task avoids blocking
the UI thread.

### New Types

```rust
pub enum UiCommand {
    SwitchBackend { backend_id: String },
    RefreshStatus,
    RefreshMetrics { backend_id: Option<String> },
    RefreshBackends,
    ReloadConfig,
}

pub enum AppEvent {
    // existing
    IpcStatus(ProxyStatus),
    IpcMetrics(MetricsSnapshot),
    IpcBackends(Vec<BackendInfo>),
    IpcError(String),
}
```

### Bridge Behavior

- UI sends `UiCommand` over a `tokio::sync::mpsc` channel.
- Bridge executes async IPC calls using `IpcClient`.
- Results are sent back as `AppEvent::*` via the UI event channel.
- UI state updates happen in the main thread only.

## Config Reload Integration

When the config file changes:

1. `ConfigWatcher` reloads `ConfigStore` and sends `AppEvent::ConfigReload`.
2. UI handles `ConfigReload` by:
   - Updating any cached UI state derived from config.
   - Sending `UiCommand::ReloadConfig` to the bridge.
3. Bridge triggers proxy-side config update:
   - Fetch latest config from `ConfigStore`.
   - Call `BackendState::update_config(new_config)`.

Notes:

- Timeouts/pool settings are read at proxy startup. Reloaded config does not
  alter `UpstreamClient` settings unless the proxy is restarted.
- `proxy.base_url` changes do not affect the already-spawned PTY process; they
  take effect on next app launch.

## Backend Switching

- UI triggers `UiCommand::SwitchBackend`.
- Bridge calls `IpcClient::switch_backend` and returns the new active backend.
- UI refreshes status + backend list on success.

## Metrics + Status Refresh

- UI periodically sends `UiCommand::RefreshStatus` and `RefreshMetrics` on tick
  (rate-limited) or when status popup is open.
- IPC responses are cached in `App` for header + popup rendering.

## Shutdown Sequence

1. UI sets `should_quit` on `Ctrl+Q`.
2. UI signals proxy shutdown via a `ProxyHandle` (new type) that calls
   `ShutdownManager::signal_shutdown()` (to be added).
3. Drop IPC client and UI↔async bridge channels.
4. Join Tokio runtime thread.
5. Terminate PTY session.

## Required Code Changes (Implementation Plan)

1. **Proxy handle**: add a manual shutdown method to `ShutdownManager` and
   expose a lightweight `ProxyHandle` with `shutdown()`.
2. **Runtime wiring**: update `src/ui/runtime.rs` to create Tokio runtime,
   start proxy + IPC tasks, and initialize the UI↔async bridge.
3. **App state**: add cached `ProxyStatus`, backend list, and metrics to `App`.
4. **Events**: extend `AppEvent` and update `handle_key` to enqueue `UiCommand`s.
5. **Config reload**: wire `ConfigReload` to `BackendState::update_config`.

## Testing Strategy

- Unit tests for the bridge: command → IPC call → AppEvent result.
- Integration test for end-to-end backend switch (UI event → IPC → backend state).
- Manual test: edit config file and verify backend list + active backend refresh.

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| UI blocks on async IPC | Bridge task avoids blocking UI thread |
| Config reload desync | Single source of truth is `ConfigStore` + `BackendState::update_config` |
| Shutdown hangs | Explicit shutdown signal and join handles |

## Open Questions

1. Do we want to restart the proxy automatically when timeouts/pool config change?
2. Should status/metrics refresh on every tick, or only when popups are open?
