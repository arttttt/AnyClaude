# Config Hot-Reload Design

**Issue**: cl-l23.3  
**Author**: polecat/obsidian  
**Date**: 2026-01-27

## Overview

Automatic configuration reload when the config file changes, without requiring application restart.

## Current State

- `Config::load()` reads `~/.config/anyclaude/config.toml` once at startup
- Config struct is `Clone` and uses serde for TOML parsing
- No file watching or hot-reload capability exists
- Application uses synchronous `std::mpsc` event system (not tokio async)
- Main event loop in `runtime.rs` processes `AppEvent` enum

## Requirements (from cl-l23.3)

1. Monitor config file for changes
2. Debounce with 200ms delay (group rapid edits)
3. Reload: read file, parse TOML, validate, apply if valid
4. Thread-safe access from multiple components
5. Notify components on successful reload
6. Handle edge cases: file deleted, permissions, invalid TOML

## Architecture

### Component Diagram

```
                    ┌─────────────────────┐
                    │   Config File       │
                    │   (~/.config/...)   │
                    └──────────┬──────────┘
                               │ notify crate watches
                               ▼
                    ┌─────────────────────┐
                    │   ConfigWatcher     │
                    │   (background       │
                    │    thread)          │
                    └──────────┬──────────┘
                               │ debounced events
                               │ reload & validate
                               ▼
┌───────────────────────────────────────────────────────┐
│                    ConfigStore                         │
│                Arc<RwLock<Config>>                    │
│                                                        │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐               │
│  │   UI    │  │  Proxy  │  │ Backend │  (readers)     │
│  └─────────┘  └─────────┘  └─────────┘               │
└───────────────────────────────────────────────────────┘
                               │
                               │ AppEvent::ConfigReload
                               ▼
                    ┌─────────────────────┐
                    │   Event Channel     │
                    │   (mpsc::Sender)    │
                    └─────────────────────┘
```

### New Types

```rust
// src/config/watcher.rs

/// Thread-safe config container with interior mutability
pub struct ConfigStore {
    inner: Arc<RwLock<Config>>,
    path: PathBuf,
}

impl ConfigStore {
    /// Create from initial config
    pub fn new(config: Config, path: PathBuf) -> Self;
    
    /// Get current config (cheap clone)
    pub fn get(&self) -> Config;
    
    /// Reload from file, returns Ok if successful
    pub fn reload(&self) -> Result<(), ConfigError>;
    
    /// Get the path being watched
    pub fn path(&self) -> &Path;
}

/// Watches config file and triggers reloads
pub struct ConfigWatcher {
    store: ConfigStore,
    _watcher: RecommendedWatcher,
}

impl ConfigWatcher {
    /// Start watching with debounce and event notification
    pub fn start(
        store: ConfigStore,
        event_tx: mpsc::Sender<AppEvent>,
        debounce_ms: u64,
    ) -> Result<Self, WatcherError>;
    
    /// Stop watching (Drop impl)
}
```

### Event System Extension

```rust
// src/ui/events.rs

pub enum AppEvent {
    Input(KeyEvent),
    Tick,
    Resize(u16, u16),
    PtyOutput,
    ConfigReload,  // NEW: config was successfully reloaded
}
```

### Integration in Runtime

```rust
// src/ui/runtime.rs

pub fn run() -> io::Result<()> {
    // Load initial config
    let config = Config::load().unwrap_or_default();
    let store = ConfigStore::new(config, Config::config_path());
    
    // Start watcher (spawns background thread)
    let _watcher = ConfigWatcher::start(
        store.clone(),
        events.sender(),
        200, // debounce ms
    );
    
    // Pass store to App for access
    let mut app = App::new(tick_rate, store.clone());
    
    // In event loop:
    match events.next(tick_rate) {
        Ok(AppEvent::ConfigReload) => app.on_config_reload(),
        // ...
    }
}
```

## Implementation Details

### File Watching

Use `notify` crate with `RecommendedWatcher`:
- Cross-platform (inotify on Linux, FSEvents on macOS, ReadDirectoryChanges on Windows)
- Built-in debouncing via `notify-debouncer-mini` or manual debounce

### Debouncing Strategy

```rust
// Manual debounce with timer thread
fn debounce_loop(
    rx: Receiver<DebouncedEvent>,
    store: ConfigStore,
    event_tx: mpsc::Sender<AppEvent>,
    debounce: Duration,
) {
    let mut last_event: Option<Instant> = None;
    
    loop {
        match rx.recv_timeout(debounce) {
            Ok(_event) => {
                last_event = Some(Instant::now());
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(last) = last_event {
                    if last.elapsed() >= debounce {
                        if store.reload().is_ok() {
                            let _ = event_tx.send(AppEvent::ConfigReload);
                        }
                        last_event = None;
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}
```

### Thread Safety

- `Arc<RwLock<Config>>` for the shared config
- `RwLock` allows multiple readers OR one writer
- Readers (`get()`) never block each other
- Writer (`reload()`) briefly blocks readers during swap

### Error Handling

| Scenario | Behavior |
|----------|----------|
| File deleted | Log warning, keep current config |
| File unreadable (permissions) | Log error, keep current config |
| Invalid TOML | Log parse error, keep current config |
| Validation fails | Log error, keep current config |
| Watcher init fails | Log error, continue without watching |

### Logging

```rust
// On successful reload
tracing::info!("Config reloaded from {:?}", path);

// On failure
tracing::warn!("Config reload failed: {}", error);
```

## Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
notify = "6.1"
# OR for built-in debounce:
notify-debouncer-mini = "0.4"
```

## File Changes

| File | Change |
|------|--------|
| `Cargo.toml` | Add `notify` dependency |
| `src/config/mod.rs` | Export `ConfigStore`, `ConfigWatcher` |
| `src/config/watcher.rs` | NEW: watcher implementation |
| `src/ui/events.rs` | Add `ConfigReload` variant |
| `src/ui/runtime.rs` | Initialize watcher, handle event |
| `src/ui/app.rs` | Add `ConfigStore` field, `on_config_reload()` |

## Testing Strategy

1. **Unit tests** for `ConfigStore`:
   - `reload()` with valid/invalid content
   - Concurrent reads during write

2. **Integration tests** for `ConfigWatcher`:
   - File modification triggers reload
   - Debounce groups rapid changes
   - Deleted file doesn't crash

3. **Manual testing**:
   - Edit config while app runs
   - Verify UI updates within 1 second

## Acceptance Criteria Mapping

| Criterion | Implementation |
|-----------|----------------|
| Changes apply within 1 second | 200ms debounce + event loop |
| Invalid TOML doesn't crash | `reload()` returns error, keeps old config |
| Rapid changes don't spam reloads | Debouncing in watcher |
| Log messages for reload status | `tracing::info/warn` calls |
| Components can subscribe | `AppEvent::ConfigReload` in event channel |

## Open Questions

1. Should we watch the parent directory instead of just the file? (handles file deletion + recreation)
2. Should we emit `ConfigReloadFailed` events to show errors in UI?
3. Should proxy components get direct `ConfigStore` access or receive events?

## Decision: Watch Parent Directory

Watch the parent directory (`~/.config/anyclaude/`) and filter for `config.toml` changes. This handles:
- File deleted and recreated (common with some editors)
- Atomic writes (write to temp file, rename)
