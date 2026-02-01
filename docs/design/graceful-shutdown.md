# Graceful Shutdown Design

## Overview

ClaudeWrapper needs a clean shutdown sequence that properly terminates all components without leaving orphan processes or losing state.

## Current State

### Components Requiring Cleanup

| Component | Current Cleanup | Issues |
|-----------|----------------|--------|
| Proxy Server | `ProxyHandle::shutdown()` | Good - has graceful shutdown |
| PTY Session | `session.shutdown()` | Reader thread has no explicit stop signal |
| Event Handler | Channel drop (implicit) | Blocks on `event::poll()`, not responsive |
| Resize Watcher | `watcher.stop()` | Good |
| Terminal Guard | RAII Drop impl | Excellent |
| Config Watcher | Drop (implicit) | Good |
| Async Runtime | `shutdown_timeout(5s)` | Good |

### Current Shutdown Flow (runtime.rs:170-174)

```rust
proxy_handle.shutdown();
let _ = pty_session.shutdown();
drop(guard);
async_runtime.shutdown_timeout(Duration::from_secs(5));
```

**Problems:**
1. No OS signal handling (SIGTERM, SIGINT) at UI level
2. Event handler thread blocks indefinitely on `event::poll()`
3. PTY reader thread has no graceful stop mechanism
4. No shutdown coordination or phase tracking
5. No visibility into shutdown progress

## Design

### Shutdown Triggers

1. **User action**: Ctrl+Q → `app.request_quit()`
2. **OS signals**: SIGTERM, SIGINT → graceful shutdown
3. **Window close**: (future) → graceful shutdown

### Phase-Based Shutdown

```
Phase 1: Signal (0-50ms)
├── Set shutdown flag (atomic)
├── Log shutdown initiation
└── Block new operations

Phase 2: Stop Input (50-100ms)
├── Stop event handler thread
├── Disable keyboard handling
└── (Optional: Show "Shutting down..." message)

Phase 3: Terminate Child (100-500ms)
├── Send SIGTERM to PTY child
├── Wait up to 300ms for graceful exit
├── Send SIGKILL if still running
└── Join reader thread

Phase 4: Close Proxy (100-500ms, parallel with Phase 3)
├── Signal proxy shutdown
├── Stop accepting new connections
├── Wait for active connections (max 500ms)
└── Force close remaining

Phase 5: Cleanup (0-50ms)
├── Drop terminal guard (restore terminal)
├── Shutdown async runtime
└── Log completion
```

### ShutdownCoordinator

New module: `src/shutdown.rs`

```rust
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ShutdownPhase {
    Running = 0,
    Signaled = 1,
    StoppingInput = 2,
    TerminatingChild = 3,
    ClosingProxy = 4,
    Cleanup = 5,
    Complete = 6,
}

pub struct ShutdownCoordinator {
    shutdown: Arc<AtomicBool>,
    phase: Arc<AtomicU8>,
    notify: Arc<Notify>,
}

impl ShutdownCoordinator {
    pub fn new() -> Self {
        Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            phase: Arc::new(AtomicU8::new(ShutdownPhase::Running as u8)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Signal shutdown start
    pub fn signal(&self) {
        if !self.shutdown.swap(true, Ordering::SeqCst) {
            tracing::info!("Graceful shutdown initiated");
            self.notify.notify_waiters();
        }
    }

    /// Check if shutdown is in progress
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Get current phase
    pub fn phase(&self) -> ShutdownPhase {
        // Convert u8 back to enum
        match self.phase.load(Ordering::SeqCst) {
            0 => ShutdownPhase::Running,
            1 => ShutdownPhase::Signaled,
            2 => ShutdownPhase::StoppingInput,
            3 => ShutdownPhase::TerminatingChild,
            4 => ShutdownPhase::ClosingProxy,
            5 => ShutdownPhase::Cleanup,
            _ => ShutdownPhase::Complete,
        }
    }

    /// Advance to next phase
    pub fn advance(&self, phase: ShutdownPhase) {
        self.phase.store(phase as u8, Ordering::SeqCst);
        tracing::debug!("Shutdown phase: {:?}", phase);
    }

    /// Wait for shutdown signal
    pub async fn wait(&self) {
        if self.is_shutting_down() {
            return;
        }
        self.notify.notified().await;
    }

    /// Create a clone for sharing
    pub fn handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            shutdown: Arc::clone(&self.shutdown),
            notify: Arc::clone(&self.notify),
        }
    }
}

/// Lightweight handle for checking shutdown state
#[derive(Clone)]
pub struct ShutdownHandle {
    shutdown: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl ShutdownHandle {
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    pub async fn wait(&self) {
        if self.is_shutting_down() {
            return;
        }
        self.notify.notified().await;
    }
}
```

### Event Handler Shutdown

Modify `src/ui/events.rs` to accept a shutdown handle:

```rust
pub struct EventHandler {
    rx: Receiver<io::Result<Event>>,
    _thread: thread::JoinHandle<()>,
}

impl EventHandler {
    pub fn new(shutdown: ShutdownHandle) -> Self {
        let (tx, rx) = mpsc::channel();

        let thread = thread::spawn(move || {
            loop {
                // Short poll timeout to check shutdown flag
                if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                    let event = event::read();
                    if tx.send(event).is_err() {
                        break; // Channel closed
                    }
                }

                // Check shutdown flag
                if shutdown.is_shutting_down() {
                    break;
                }
            }
        });

        Self { rx, _thread: thread }
    }
}
```

### PTY Session Shutdown

Modify `src/pty/session.rs` for graceful child termination:

```rust
impl PtySession {
    pub fn shutdown(&mut self) -> Result<(), Box<dyn Error>> {
        // Close stdin to signal EOF to child
        self.handle.close_writer();

        // Give child a chance to exit gracefully
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            if let Some(pid) = self.child.id() {
                let pid = Pid::from_raw(pid as i32);
                let _ = kill(pid, Signal::SIGTERM);
            }
        }

        // Wait with timeout
        let deadline = std::time::Instant::now() + Duration::from_millis(300);
        loop {
            match self.child.try_wait()? {
                Some(_) => break,
                None if std::time::Instant::now() >= deadline => {
                    // Force kill
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }

        // Join reader thread
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }

        Ok(())
    }
}
```

### OS Signal Handling

Add to `src/ui/runtime.rs`:

```rust
use tokio::signal;

async fn setup_signal_handler(shutdown: ShutdownHandle) {
    #[cfg(unix)]
    {
        let mut sigterm = signal::unix::signal(
            signal::unix::SignalKind::terminate()
        ).expect("Failed to install SIGTERM handler");

        tokio::select! {
            _ = signal::ctrl_c() => {
                tracing::info!("Received SIGINT");
            }
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM");
            }
        }

        shutdown.signal();
    }

    #[cfg(not(unix))]
    {
        signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
        shutdown.signal();
    }
}
```

### Updated Runtime Shutdown

```rust
pub fn run() -> io::Result<()> {
    // ... initialization ...

    let shutdown = ShutdownCoordinator::new();
    let shutdown_handle = shutdown.handle();

    // Spawn OS signal handler in async runtime
    async_runtime.spawn(setup_signal_handler(shutdown_handle.clone()));

    // Pass shutdown handle to event handler
    let events = EventHandler::new(shutdown_handle.clone());

    // ... main loop ...

    // When app.should_quit() or shutdown signaled:
    shutdown.signal();

    // Phase 2: Stop input
    shutdown.advance(ShutdownPhase::StoppingInput);
    drop(events); // Stop event handler

    // Phase 3 & 4: Terminate child and close proxy (parallel)
    shutdown.advance(ShutdownPhase::TerminatingChild);
    let pty_result = std::thread::scope(|s| {
        let proxy_handle = s.spawn(|| {
            proxy_handle.shutdown();
        });
        let pty_result = pty_session.shutdown();
        let _ = proxy_handle.join();
        pty_result
    });

    // Phase 5: Cleanup
    shutdown.advance(ShutdownPhase::Cleanup);
    drop(guard); // Restore terminal
    async_runtime.shutdown_timeout(Duration::from_secs(2));

    shutdown.advance(ShutdownPhase::Complete);
    tracing::info!("Shutdown complete");

    Ok(())
}
```

## File Changes Summary

| File | Changes |
|------|---------|
| `src/shutdown.rs` | New file - ShutdownCoordinator |
| `src/lib.rs` | Add `pub mod shutdown;` |
| `src/ui/runtime.rs` | Integrate ShutdownCoordinator, add OS signal handling |
| `src/ui/events.rs` | Accept ShutdownHandle, check flag in poll loop |
| `src/pty/session.rs` | Graceful SIGTERM before SIGKILL |

## Testing Strategy

1. **Unit tests**: Test ShutdownCoordinator state transitions
2. **Integration tests**:
   - Verify Ctrl+Q triggers clean shutdown
   - Verify SIGTERM triggers clean shutdown
   - Verify no orphan processes after shutdown
   - Verify terminal restored correctly
3. **Manual tests**:
   - Run with active proxy connections, verify drain
   - Run with Claude session, verify clean exit

## Acceptance Criteria

- [ ] Ctrl+Q triggers graceful shutdown
- [ ] SIGTERM triggers graceful shutdown
- [ ] No orphan PTY processes after shutdown
- [ ] No hanging proxy connections
- [ ] Terminal restored to normal mode
- [ ] Shutdown completes within 5 seconds
- [ ] Shutdown progress visible in logs
