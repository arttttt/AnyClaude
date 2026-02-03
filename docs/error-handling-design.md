# Error Handling and Recovery — Design Document

## Problem Statement

The anyclaude application needs comprehensive error handling across six components to:
1. Gracefully handle failures without crashing
2. Provide clear user feedback through UI indicators
3. Attempt automatic recovery where possible
4. Degrade gracefully when full recovery isn't possible

## Current State Analysis

### Already Implemented
| Component | Error Type | Status |
|-----------|-----------|--------|
| Proxy | `ProxyError` enum with status codes | ✅ Complete |
| Config | `ConfigError` enum | ✅ Complete |
| Backend | `BackendError` enum | ✅ Complete |
| IPC | `IpcError` enum | ✅ Complete |
| Upstream | Retry with exponential backoff | ✅ Complete |
| Header | Status indicator (🟢/🔴/⚪) | ✅ Complete |
| Diagnostics | Network Diagnostics panel (Ctrl+S) | ✅ Complete |
| Shutdown | `ShutdownCoordinator` | ✅ Complete |

### Gaps to Address
| Gap | Impact | Priority |
|-----|--------|----------|
| No unified error registry for UI | UI can't display comprehensive status | High |
| PTY crash handling is silent | User unaware of child process death | High |
| No recovery notifications | User doesn't know retry status | Medium |
| Config errors lack location info | Hard to debug TOML issues | Medium |
| IPC errors only shown in popups | User may miss errors | Medium |
| No degradation mode tracking | Can't show partial functionality | Low |

## Architecture Design

### 1. Error Classification

```
┌─────────────────────────────────────────────────────────────┐
│                     Error Classification                     │
├─────────────────────────────────────────────────────────────┤
│ CRITICAL (require immediate user notification)              │
│   • Proxy cannot reach any backend                          │
│   • PTY process crashes/exits unexpectedly                  │
│   • Config file missing or invalid                          │
│   • All backend credentials invalid                         │
├─────────────────────────────────────────────────────────────┤
│ RECOVERABLE (auto-retry with backoff)                       │
│   • Temporary backend connection timeout                    │
│   • Single backend failure with fallback available          │
│   • Network transient errors                                │
│   • Rate limiting responses (429)                           │
├─────────────────────────────────────────────────────────────┤
│ DEGRADATION (continue with reduced functionality)           │
│   • Metrics collection failure                              │
│   • Clipboard access denied                                 │
│   • Config hot-reload failure                               │
│   • Backend switch partially failed                         │
└─────────────────────────────────────────────────────────────┘
```

### 2. New Types

#### `src/error.rs` — Application-level Error Registry

```rust
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

/// Severity level for application errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ErrorSeverity {
    /// Informational - no action needed
    Info,
    /// Warning - degraded but functional
    Warning,
    /// Error - feature unavailable
    Error,
    /// Critical - application unstable
    Critical,
}

/// Category of error for filtering/display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// PTY/child process issues
    Process,
    /// Network/proxy/upstream issues
    Network,
    /// Configuration issues
    Config,
    /// Backend issues
    Backend,
    /// IPC communication issues
    Ipc,
    /// General system issues
    System,
}

/// An application error with user-friendly messaging.
#[derive(Debug, Clone)]
pub struct AppError {
    /// Unique identifier for this error instance
    pub id: u64,
    /// When the error occurred
    pub timestamp: SystemTime,
    /// Severity level
    pub severity: ErrorSeverity,
    /// Error category
    pub category: ErrorCategory,
    /// User-friendly message (shown in header/footer)
    pub message: String,
    /// Technical details (shown in diagnostics panel)
    pub details: Option<String>,
    /// Recovery suggestion for user
    pub recovery_hint: Option<String>,
    /// Whether this error has been acknowledged
    pub acknowledged: bool,
}

/// Recovery state for automatic retry operations.
#[derive(Debug, Clone)]
pub struct RecoveryState {
    /// What is being recovered
    pub operation: String,
    /// Current attempt number
    pub attempt: u32,
    /// Maximum attempts before giving up
    pub max_attempts: u32,
    /// When next retry will occur
    pub next_retry: Option<SystemTime>,
    /// Whether recovery succeeded
    pub succeeded: bool,
}

/// Thread-safe error registry for the application.
#[derive(Clone)]
pub struct ErrorRegistry {
    inner: Arc<RwLock<ErrorRegistryInner>>,
}

struct ErrorRegistryInner {
    /// Next error ID
    next_id: u64,
    /// Recent errors (ring buffer)
    errors: VecDeque<AppError>,
    /// Maximum errors to retain
    capacity: usize,
    /// Current recovery operations in progress
    recoveries: Vec<RecoveryState>,
    /// Overall system health
    healthy: bool,
    /// Reason for unhealthy state
    unhealthy_reason: Option<String>,
}

impl ErrorRegistry {
    pub fn new(capacity: usize) -> Self;

    /// Record a new error
    pub fn record(&self, severity: ErrorSeverity, category: ErrorCategory,
                  message: impl Into<String>) -> u64;

    /// Record error with details
    pub fn record_with_details(&self, severity: ErrorSeverity, category: ErrorCategory,
                               message: impl Into<String>, details: impl Into<String>) -> u64;

    /// Get the most recent critical/error (for header display)
    pub fn current_error(&self) -> Option<AppError>;

    /// Get all errors for diagnostics panel
    pub fn all_errors(&self) -> Vec<AppError>;

    /// Get errors by category
    pub fn errors_by_category(&self, category: ErrorCategory) -> Vec<AppError>;

    /// Acknowledge an error (removes from header display)
    pub fn acknowledge(&self, error_id: u64);

    /// Clear errors older than duration
    pub fn clear_old(&self, older_than: Duration);

    /// Start tracking a recovery operation
    pub fn start_recovery(&self, operation: impl Into<String>, max_attempts: u32);

    /// Update recovery attempt
    pub fn update_recovery(&self, operation: &str, attempt: u32, next_retry: Option<SystemTime>);

    /// Mark recovery as succeeded
    pub fn recovery_succeeded(&self, operation: &str);

    /// Mark recovery as failed
    pub fn recovery_failed(&self, operation: &str);

    /// Get current recovery operations
    pub fn active_recoveries(&self) -> Vec<RecoveryState>;

    /// Check if system is healthy
    pub fn is_healthy(&self) -> bool;

    /// Set system health status
    pub fn set_health(&self, healthy: bool, reason: Option<String>);
}
```

### 3. Component Error Integration

#### PTY Manager — Process Monitoring

Add to `src/pty/session.rs`:

```rust
/// Error types for PTY operations
pub enum PtyError {
    /// Child process exited with code
    ProcessExited { exit_code: i32 },
    /// Child process was killed by signal
    ProcessKilled { signal: i32 },
    /// Spawn failed
    SpawnFailed { command: String, error: String },
    /// Resize failed
    ResizeFailed { error: String },
    /// Read error from PTY
    ReadError { error: String },
    /// Write error to PTY
    WriteError { error: String },
}

impl PtyError {
    /// User-friendly message for display
    pub fn user_message(&self) -> &'static str {
        match self {
            PtyError::ProcessExited { .. } => "Claude Code has exited",
            PtyError::ProcessKilled { .. } => "Claude Code was terminated",
            PtyError::SpawnFailed { .. } => "Failed to start Claude Code",
            PtyError::ResizeFailed { .. } => "Terminal resize failed",
            PtyError::ReadError { .. } => "Lost connection to Claude Code",
            PtyError::WriteError { .. } => "Cannot send input to Claude Code",
        }
    }
}
```

Update reader thread to detect exit vs crash:
```rust
// In reader thread
match reader.read(&mut buffer) {
    Ok(0) => {
        // EOF - check child status
        break;
    }
    Err(e) => {
        // Report error to registry
        let _ = notifier.send(AppEvent::PtyError(PtyError::ReadError {
            error: e.to_string(),
        }));
        break;
    }
}
```

#### Proxy — Enhanced Error Reporting

Update `src/proxy/upstream.rs` to report to ErrorRegistry:

```rust
impl UpstreamClient {
    pub async fn forward(
        &self,
        req: Request<Body>,
        backend_state: &BackendState,
        backend_override: Option<String>,
        span: RequestSpan,
        observability: ObservabilityHub,
        error_registry: ErrorRegistry,  // NEW
    ) -> Result<Response<Body>, ProxyError> {
        // ... existing code ...

        match send_result {
            Ok(response) => break response,
            Err(err) => {
                let should_retry = err.is_connect() || err.is_timeout();
                if should_retry && attempt < self.pool_config.max_retries {
                    // Report recovery attempt
                    error_registry.update_recovery(
                        "backend_connection",
                        attempt + 1,
                        Some(SystemTime::now() + backoff),
                    );
                    // ... existing retry logic ...
                } else {
                    // Report final failure
                    error_registry.record_with_details(
                        ErrorSeverity::Error,
                        ErrorCategory::Network,
                        format!("Connection to {} failed", backend.name),
                        err.to_string(),
                    );
                    error_registry.recovery_failed("backend_connection");
                }
            }
        }
    }
}
```

#### Config — Location-aware Error Messages

Update `src/config/loader.rs`:

```rust
impl ConfigError {
    /// User-friendly message for header display
    pub fn user_message(&self) -> String {
        match self {
            ConfigError::ReadError { path, .. } => {
                format!("Cannot read config: {}", path.display())
            }
            ConfigError::ParseError { path, source } => {
                // Extract line/column from TOML error if available
                if let Some(span) = source.span() {
                    format!(
                        "Config error at line {}: {}",
                        span.start,
                        source.message()
                    )
                } else {
                    format!("Invalid config: {}", source.message())
                }
            }
            ConfigError::ValidationError { message } => {
                message.clone()
            }
        }
    }

    /// Detailed message for diagnostics panel
    pub fn details(&self) -> String {
        match self {
            ConfigError::ReadError { path, source } => {
                format!(
                    "File: {}\nError: {}",
                    path.display(),
                    source
                )
            }
            ConfigError::ParseError { path, source } => {
                format!(
                    "File: {}\nError: {}\n\nCheck TOML syntax at the indicated location.",
                    path.display(),
                    source
                )
            }
            ConfigError::ValidationError { message } => {
                format!("{}\n\nEdit config.toml to fix this issue.", message)
            }
        }
    }
}
```

### 4. UI Integration

#### Header Status Display

Update `src/ui/header.rs`:

```rust
impl Header {
    pub fn widget(
        &self,
        status: Option<&ProxyStatus>,
        error_registry: &ErrorRegistry,
    ) -> Paragraph<'static> {
        // Determine status icon and color
        let (icon, status_color, status_text) = if let Some(error) = error_registry.current_error() {
            match error.severity {
                ErrorSeverity::Critical => ("🔴", STATUS_ERROR, Some(error.message.clone())),
                ErrorSeverity::Error => ("🔴", STATUS_ERROR, Some(error.message.clone())),
                ErrorSeverity::Warning => ("🟡", STATUS_WARNING, None),
                ErrorSeverity::Info => ("🟢", STATUS_OK, None),
            }
        } else if let Some(recovery) = error_registry.active_recoveries().first() {
            ("🟡", STATUS_WARNING, Some(format!(
                "Retrying... (attempt {}/{})",
                recovery.attempt,
                recovery.max_attempts
            )))
        } else {
            match status {
                Some(s) if s.healthy => ("🟢", STATUS_OK, None),
                Some(_) => ("🔴", STATUS_ERROR, Some("Connection error".to_string())),
                None => ("⚪", STATUS_ERROR, None),
            }
        };

        // Build header line with optional error message
        let mut spans = vec![
            Span::styled(" ", text_style),
            Span::styled(icon, status_style),
            Span::styled(" ", text_style),
        ];

        if let Some(msg) = status_text {
            spans.push(Span::styled(msg, Style::default().fg(status_color)));
            spans.push(Span::styled(" │ ", text_style));
        }

        // ... rest of backend/requests/uptime display
    }
}
```

Add `STATUS_WARNING` to theme:
```rust
pub const STATUS_WARNING: Color = Color::Rgb(0xf5, 0x9e, 0x0b); // amber-500
```

#### Network Diagnostics Panel — Error Details

Update diagnostics panel in `src/ui/render.rs`:

```rust
PopupKind::Status => {
    let mut lines = Vec::new();

    // ... existing backend info ...

    // Add error section if there are errors
    let errors = error_registry.errors_by_category(ErrorCategory::Network);
    if !errors.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  Recent Errors:", Style::default().fg(STATUS_ERROR)),
        ]));

        for error in errors.iter().take(3) {
            let time = format_relative_time(error.timestamp);
            lines.push(Line::from(vec![
                Span::styled(format!("    [{time}] "), Style::default().fg(HEADER_TEXT)),
                Span::styled(&error.message, Style::default().fg(STATUS_ERROR)),
            ]));

            if let Some(details) = &error.details {
                for line in details.lines().take(2) {
                    lines.push(Line::from(vec![
                        Span::styled(format!("      {line}"), Style::default().fg(HEADER_TEXT)),
                    ]));
                }
            }
        }
    }

    // Add recovery section if recovering
    let recoveries = error_registry.active_recoveries();
    if !recoveries.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  Recovery:", Style::default().fg(STATUS_WARNING)),
        ]));

        for recovery in &recoveries {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("    {} (attempt {}/{})",
                            recovery.operation,
                            recovery.attempt,
                            recovery.max_attempts),
                    Style::default().fg(STATUS_WARNING),
                ),
            ]));
        }
    }

    ("Network Diagnostics", lines)
}
```

### 5. Graceful Degradation

Add degradation state tracking:

```rust
/// Features that can be degraded
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    Metrics,
    Clipboard,
    ConfigHotReload,
    BackendSwitch,
}

impl ErrorRegistry {
    /// Mark a feature as degraded
    pub fn degrade_feature(&self, feature: Feature, reason: impl Into<String>);

    /// Check if feature is available
    pub fn is_feature_available(&self, feature: Feature) -> bool;

    /// Restore a feature
    pub fn restore_feature(&self, feature: Feature);
}
```

Usage in runtime:
```rust
// In clipboard initialization
let mut clipboard = match ClipboardHandler::new() {
    Ok(handler) => Some(handler),
    Err(e) => {
        error_registry.degrade_feature(
            Feature::Clipboard,
            format!("Clipboard unavailable: {}", e),
        );
        error_registry.record(
            ErrorSeverity::Warning,
            ErrorCategory::System,
            "Clipboard unavailable (headless mode?)",
        );
        None
    }
};
```

### 6. Event Flow

```
┌──────────────────────────────────────────────────────────────────────────┐
│                           Error Event Flow                                │
└──────────────────────────────────────────────────────────────────────────┘

  Component Error                   ErrorRegistry              UI
       │                                 │                      │
       │  record(severity, msg)          │                      │
       ├────────────────────────────────>│                      │
       │                                 │                      │
       │                                 │  current_error()     │
       │                                 │<─────────────────────┤
       │                                 │                      │
       │                                 │  AppError            │
       │                                 ├─────────────────────>│
       │                                 │                      │
       │                                 │      ┌───────────────┴───────────┐
       │                                 │      │  Header: 🔴 Error message │
       │                                 │      │  Diagnostics: Details     │
       │                                 │      └───────────────────────────┘
       │                                 │                      │
       │  start_recovery("op", 3)        │                      │
       ├────────────────────────────────>│                      │
       │                                 │                      │
       │                                 │      ┌───────────────┴───────────┐
       │                                 │      │  Header: 🟡 Retrying...   │
       │                                 │      └───────────────────────────┘
       │                                 │                      │
       │  recovery_succeeded("op")       │                      │
       ├────────────────────────────────>│                      │
       │                                 │                      │
       │                                 │      ┌───────────────┴───────────┐
       │                                 │      │  Header: 🟢 Restored      │
       │                                 │      │  (brief, then normal)     │
       │                                 │      └───────────────────────────┘
```

### 7. Implementation Plan

#### Phase 1: Core Infrastructure
1. Create `src/error.rs` with `ErrorRegistry`, `AppError`, `ErrorSeverity`, `ErrorCategory`
2. Add `ErrorRegistry` to `App` struct
3. Update `Header` to show current error
4. Update `STATUS_WARNING` color in theme

#### Phase 2: PTY Error Handling
1. Add `PtyError` enum to `src/pty/session.rs`
2. Update reader thread to detect and report errors
3. Add `AppEvent::PtyError` variant
4. Handle PTY errors in runtime event loop

#### Phase 3: Proxy/Network Errors
1. Integrate `ErrorRegistry` into `UpstreamClient`
2. Report connection failures with retry status
3. Report final failures after exhausting retries

#### Phase 4: Config Error Details
1. Update `ConfigError` with `user_message()` and `details()`
2. Report config errors to registry on startup and hot-reload
3. Show config errors in diagnostics panel with line numbers

#### Phase 5: Degradation Handling
1. Add `Feature` enum and degradation tracking
2. Degrade gracefully on clipboard, metrics, hot-reload failures
3. Show degraded features in diagnostics panel

#### Phase 6: Recovery Notifications
1. Implement recovery state tracking
2. Show "Retrying..." in header during recovery
3. Show "Connection restored" briefly after recovery

## Acceptance Criteria

- [ ] All components implement graceful error handling without panics
- [ ] User-friendly error messages displayed in UI (no technical stack traces)
- [ ] Header shows error status indicator (red) when system degraded
- [ ] Network Diagnostics panel (Ctrl+S) displays detailed error information
- [ ] Auto-recovery with exponential backoff for transient failures
- [ ] Graceful degradation: partial failures do not crash entire application
- [ ] Config errors show file path and line number in diagnostics
- [ ] Backend failures show which backend failed and available alternatives
- [ ] Recovery instructions provided to user when manual intervention needed
- [ ] Error logging for debugging without exposing sensitive data (API keys)

## Files to Create/Modify

| File | Action | Description |
|------|--------|-------------|
| `src/error.rs` | Create | ErrorRegistry, AppError, severity/category types |
| `src/ui/app.rs` | Modify | Add ErrorRegistry field |
| `src/ui/header.rs` | Modify | Display current error from registry |
| `src/ui/render.rs` | Modify | Show errors in diagnostics panel |
| `src/ui/theme.rs` | Modify | Add STATUS_WARNING color |
| `src/ui/runtime.rs` | Modify | Initialize ErrorRegistry, handle PtyError events |
| `src/ui/events.rs` | Modify | Add PtyError, ConfigError event variants |
| `src/pty/session.rs` | Modify | Add PtyError enum, detect process exit |
| `src/config/loader.rs` | Modify | Add user_message(), details() methods |
| `src/proxy/upstream.rs` | Modify | Report errors/recovery to registry |
| `src/lib.rs` | Modify | Export error module |

## Testing Strategy

1. **Unit tests** for ErrorRegistry operations
2. **Integration tests** for error event flow
3. **Manual testing**:
   - Kill Claude Code process → verify header shows error
   - Disconnect network → verify retry notification
   - Invalid config → verify error with line number
   - Remove API key → verify backend shows as unconfigured
