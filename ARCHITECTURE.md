# Claude Wrapper — Architecture

TUI wrapper for Claude Code with hot-swappable backend support.

## Concept

Embed Claude Code via PTY, run local reverse proxy for routing requests to backends. Switch backends on the fly without restarting Claude Code.

## Principles

- **SOLID** — single responsibility, open/closed, etc.
- **DRY** — don't repeat yourself
- **KISS** — keep it simple
- **YAGNI** — you aren't gonna need it

## Modules

### 1. PTY Manager (`src/pty/`)
Create and manage PTY for Claude Code.
- Process spawn
- I/O streaming
- Resize handling
- VT parsing (termwiz)

### 2. UI Components (`src/ui/`)
Each component in its own file:
- `header.rs` — backend status, model, tokens
- `footer.rs` — hotkey hints
- `terminal.rs` — PTY view
- `popup.rs` — modal windows (backend menu, diagnostics)
- `theme.rs` — color palette
- `app.rs` — application state

### 3. Event Router (`src/ui/events.rs`)
Event handling:
- Keyboard events → command dispatch
- Resize events
- PTY output events
- Tick events

### 4. Config Management (`src/config/`)
Load/save settings:
- Backend configurations (TOML)
- User preferences
- Runtime state

### 5. Proxy Server (`src/proxy/`)
HTTP reverse proxy:
- `router.rs` — hot-swap backends
- `upstream.rs` — connection pooling

### 6. Backend Manager (`src/backend/`)
Backend management:
- CRUD operations
- Config validation
- Health checks

### 7. IPC Layer (`src/ipc/`)
TUI ↔ Proxy communication:
- Unix socket
- Switch commands

### 8. Metrics (`src/metrics/`)
Statistics collection:
- Request count
- Latency
- Errors
- Token usage

## Tests

All tests live in `tests/`. No `#[cfg(test)]` in source files. Shared mocks and builders go in a common module. Enforced by `just check`.

## Module Dependencies

```
main.rs
    └── ui/mod.rs (run loop)
            ├── pty/ (PTY Manager)
            ├── config/ (settings)
            ├── proxy/ (backend routing)
            └── ui/* (components)
```

Minimal coupling. Each module communicates through clear interfaces (traits).
