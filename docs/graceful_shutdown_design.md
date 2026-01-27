# Graceful Shutdown Design

## Overview

Implement graceful shutdown for the ProxyServer to safely terminate without interrupting active requests.

## Requirements Summary

- Handle SIGTERM and SIGINT signals
- Stop accepting new connections on shutdown
- Wait for active connections to complete (10s timeout)
- Clean up resources (listener, port, runtime)
- Log shutdown progress to stdout

## Architecture

### Components

1. **ShutdownManager** - New module
   - Signal handling (cross-platform)
   - Shutdown state coordination
   - Connection tracking
   - Timeout management

2. **ProxyServer** - Modified
   - Integration with ShutdownManager
   - Stop accepting new connections
   - Track active connections

### Signal Flow

```
SIGTERM/SIGINT
    ↓
ShutdownManager
    ↓
Set shutdown flag
    ↓
Stop TcpListener.accept()
    ↓
Wait for active connections (with timeout)
    ↓
Force close if timeout
    ↓
Log completion
    ↓
Exit
```

## Implementation Details

### 1. ShutdownManager Module

**Location:** `src/proxy/shutdown.rs`

**Responsibilities:**
- Register signal handlers (tokio::signal)
- Provide atomic shutdown flag (AtomicBool)
- Track active connection count (AtomicUsize)
- Provide wait/shutdown methods

**API:**
```rust
pub struct ShutdownManager {
    shutdown: Arc<AtomicBool>,
    active_connections: Arc<AtomicUsize>,
}

impl ShutdownManager {
    pub fn new() -> Self;
    pub async fn wait_for_signal(&self) -> Result<(), Error>;
    pub fn is_shutting_down(&self) -> bool;
    pub fn increment_connections(&self);
    pub fn decrement_connections(&self);
    pub async fn wait_for_connections(&self, timeout: Duration) -> bool;
}
```

### 2. ProxyServer Modifications

**Changes to `src/proxy/mod.rs`:**

1. Add ShutdownManager field
2. Modify `run()` method:
   - Spawn signal listener task
   - Check shutdown flag before accepting new connections
   - Track connection count (increment on accept, decrement on complete)

**Control Flow:**
```rust
pub async fn run(&self) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(self.addr).await?;
    let shutdown = self.shutdown.clone();
    
    // Spawn signal handler
    tokio::spawn(async move {
        shutdown.wait_for_signal().await;
        // Will cause accept() to return error when listener closed
    });
    
    // Accept loop with shutdown check
    loop {
        if shutdown.is_shutting_down() {
            break; // Stop accepting new connections
        }
        
        let (stream, _) = listener.accept().await?;
        // Track connection, spawn handler
    }
    
    // Wait for active connections with timeout
    shutdown.wait_for_connections(Duration::from_secs(10)).await;
    
    Ok(())
}
```

### 3. Signal Handling

**Unix/Linux/macOS:**
```rust
tokio::signal::ctrl_c().await?;  // SIGINT
tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    .unwrap()
    .recv()
    .await;  // SIGTERM
```

**Windows:**
```rust
tokio::signal::ctrl_c().await?;  // Ctrl+C
```

Note: tokio::signal handles Ctrl+C uniformly across platforms. SIGTERM is Unix-only.

### 4. Connection Tracking

**Strategy:**
- Increment counter before spawning handler task
- Decrement in drop or task completion
- Use Arc<AtomicUsize> for thread-safe counter

**Implementation:**
```rust
pub async fn handle_connection(&self) {
    self.shutdown.increment_connections();
    
    let _guard = scopeguard::guard(self.shutdown.clone(), |shutdown| {
        shutdown.decrement_connections();
    });
    
    // Handle request...
}
```

### 5. Logging

**Messages (in order):**
1. "Shutting down gracefully..."
2. "Waiting for 0 active connections..."
3. "Server stopped" OR "Forced shutdown after timeout"

**Where:** println! in ShutdownManager and ProxyServer

## Error Handling

- Signal handler errors: log and continue (shutdown signal received anyway)
- Connection errors: don't block shutdown
- Timeout: force exit (Tokio runtime will terminate tasks)

## Testing Strategy

### Unit Tests
1. ShutdownManager signal handling
2. Connection counter accuracy
3. Timeout logic

### Integration Tests
1. Send SIGTERM, verify graceful shutdown
2. Make active request during shutdown, verify completion
3. Verify timeout forces shutdown
4. Immediate restart after shutdown (port free)

## Dependencies

**Already in Cargo.toml:**
- tokio (features: macros, rt-multi-thread, io-util) ✓
- signal-hook (not needed - use tokio::signal)

**New dependencies:**
- None (tokio provides everything needed)

## Edge Cases

1. **Multiple signals:** First signal triggers shutdown, subsequent ignored
2. **No active connections:** Immediate exit
3. **Listener bind fails:** Existing error handling sufficient
4. **Tokio runtime panic:** Unavoidable, but rare

## Success Criteria

1. Server exits without panic on SIGTERM/SIGINT
2. Active request completes before server exit
3. After 10s timeout, server forces shutdown
4. Port is freed for immediate restart
5. All three log messages appear in stdout

## Next Steps

After design approval:
1. Create `src/proxy/shutdown.rs`
2. Modify `src/proxy/mod.rs` to integrate
3. Write unit tests for ShutdownManager
4. Write integration tests for full shutdown flow
5. Verify all acceptance criteria
