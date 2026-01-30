# IPC Layer for TUI <-> Proxy Communication — Design Document

## Overview

This document defines the in-process IPC layer used by the TUI to send commands to the proxy runtime and receive responses. The IPC layer uses Tokio channels (mpsc + oneshot) to connect async tasks within a single process.

## Requirements

- TUI can send commands to proxy and receive replies.
- Commands supported:
  - SwitchBackend(backend_id)
  - GetStatus
  - GetMetrics
  - ListBackends
- TUI must await responses with a 1 second timeout.
- If the IPC receiver closes, the app should shut down gracefully.
- If the IPC sender closes, proxy keeps running.
- No panics on channel closure.

## Current State

- `src/ipc/mod.rs` is a placeholder.
- TUI and proxy run as separate async tasks in one process.
- Proxy owns `BackendState` and `ObservabilityHub` needed to answer IPC requests.

## Design Decisions

### Decision 1: Use Tokio mpsc + oneshot

**Choice**: A bounded `tokio::sync::mpsc` channel (capacity 16) carries commands. Each command includes a `tokio::sync::oneshot::Sender` to return the response.

**Rationale**:
- Single-process, async-friendly, and already a project dependency.
- Bounded channel prevents unbounded memory growth.
- oneshot keeps responses tied to requests.

### Decision 2: Explicit command and response enums

**Choice**: Define `IpcCommand` and `IpcResponse` enums. Each command includes a responder for its response type.

**Rationale**:
- Keeps IPC surface self-documenting.
- Avoids brittle dynamic typing.
- Allows per-command response payloads.

### Decision 3: Proxy owns the receiver

**Choice**: The proxy runtime owns the `mpsc::Receiver<IpcCommand>` and processes commands in a dedicated loop. The TUI owns the `mpsc::Sender<IpcCommand>`.

**Rationale**:
- Proxy already owns state required to fulfill commands.
- TUI never needs to mutate proxy state directly.

### Decision 4: TUI-side timeout handling

**Choice**: TUI uses `tokio::time::timeout(Duration::from_secs(1), recv)` when awaiting the oneshot response.

**Rationale**:
- Prevents UI blocking on slow proxy work.
- Centralized timeout logic in the caller.

## Architecture

```
┌──────────────┐    mpsc (bounded)     ┌──────────────────┐
│     TUI      │ ───────────────────▶ │    Proxy Task    │
│  IpcClient   │                      │   IpcServerLoop  │
└──────┬───────┘                      └────────┬─────────┘
       │ oneshot reply                           │
       └─────────────────────────────────────────┘
```

## Data Structures

### Commands

```rust
pub enum IpcCommand {
    SwitchBackend {
        backend_id: String,
        respond_to: oneshot::Sender<Result<String, BackendError>>,
    },
    GetStatus {
        respond_to: oneshot::Sender<ProxyStatus>,
    },
    GetMetrics {
        backend_id: Option<String>,
        respond_to: oneshot::Sender<MetricsSnapshot>,
    },
    ListBackends {
        respond_to: oneshot::Sender<Vec<BackendInfo>>,
    },
}
```

### Responses

```rust
pub struct ProxyStatus {
    pub active_backend: String,
    pub uptime_seconds: u64,
    pub total_requests: u64,
    pub healthy: bool,
}

pub struct BackendInfo {
    pub id: String,
    pub display_name: String,
    pub is_active: bool,
    pub is_configured: bool,
}
```

### IPC handles

```rust
pub struct IpcClient {
    sender: mpsc::Sender<IpcCommand>,
}

pub struct IpcServer {
    receiver: mpsc::Receiver<IpcCommand>,
}
```

`IpcLayer::new()` returns `(IpcClient, IpcServer)` with a bounded channel of size 16.

## Command Handling Logic

### SwitchBackend

- Calls `BackendState::switch_backend()`.
- On success, returns new active backend ID.
- On error, returns `BackendError`.

### GetStatus

- Uses `BackendState::get_active_backend()`.
- Uses proxy uptime tracker (start time stored in proxy runtime).
- Uses observability aggregate counts for `total_requests`.
- `healthy` is true if proxy task is running and no shutdown signal received.

### GetMetrics

- Calls `ObservabilityHub::snapshot()`.
- If `backend_id` provided, filter snapshot to a single backend and relevant recent records.

### ListBackends

- Uses `BackendState::get_config()` for full backend list.
- For each backend:
  - `is_active` if `backend.name == active_backend`.
  - `is_configured` via `backend.is_configured()` from `config::credentials`.

## Error Handling

- If `mpsc::Receiver` closes: proxy treats as UI gone and continues to serve requests.
- If `mpsc::Sender` closes: TUI treats this as proxy shutdown and initiates app shutdown.
- If `oneshot::Sender` fails (receiver dropped): proxy ignores the response.
- TUI timeouts (1s) surface as non-fatal errors to the UI.

## Implementation Plan

1. Implement IPC types in `src/ipc/mod.rs` (commands, responses, client/server, errors).
2. Add a proxy IPC loop task that owns `IpcServer` and handles commands.
3. Wire TUI input handlers to use `IpcClient` for Ctrl+B / Ctrl+S actions.
4. Add status and metrics display in UI with timeout-safe calls.

## Acceptance Criteria Mapping

- TUI sends command and receives response: mpsc + oneshot.
- SwitchBackend verified via GetStatus: uses BackendState.
- GetMetrics returns ObservabilityHub snapshot.
- 1s timeout enforced in TUI.
- Channel closures handled without panic.

## Open Questions

None.
