//! Application-level error handling and registry.
//!
//! Provides a centralized error tracking system for the TUI, with support for:
//! - Error severity classification
//! - User-friendly messages with technical details
//! - Recovery operation tracking
//! - Graceful degradation state

use std::collections::{HashSet, VecDeque};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// Features that can be degraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Metrics collection
    Metrics,
    /// Clipboard access
    Clipboard,
    /// Config hot-reload
    ConfigHotReload,
    /// Backend switching
    BackendSwitch,
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
    /// Degraded features
    degraded_features: HashSet<Feature>,
}

impl ErrorRegistry {
    /// Create a new error registry with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ErrorRegistryInner {
                next_id: 1,
                errors: VecDeque::with_capacity(capacity),
                capacity,
                recoveries: Vec::new(),
                healthy: true,
                unhealthy_reason: None,
                degraded_features: HashSet::new(),
            })),
        }
    }

    /// Record a new error.
    pub fn record(
        &self,
        severity: ErrorSeverity,
        category: ErrorCategory,
        message: impl Into<String>,
    ) -> u64 {
        self.record_with_details(severity, category, message, None::<String>)
    }

    /// Record error with details.
    pub fn record_with_details(
        &self,
        severity: ErrorSeverity,
        category: ErrorCategory,
        message: impl Into<String>,
        details: Option<impl Into<String>>,
    ) -> u64 {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        let id = inner.next_id;
        inner.next_id += 1;

        let error = AppError {
            id,
            timestamp: SystemTime::now(),
            severity,
            category,
            message: message.into(),
            details: details.map(Into::into),
            recovery_hint: None,
            acknowledged: false,
        };

        // Update health status for critical/error severity
        if severity >= ErrorSeverity::Error {
            inner.healthy = false;
            inner.unhealthy_reason = Some(error.message.clone());
        }

        // Ring buffer push
        if inner.errors.len() == inner.capacity {
            inner.errors.pop_front();
        }
        inner.errors.push_back(error);

        id
    }

    /// Record error with recovery hint.
    pub fn record_with_hint(
        &self,
        severity: ErrorSeverity,
        category: ErrorCategory,
        message: impl Into<String>,
        details: Option<impl Into<String>>,
        recovery_hint: impl Into<String>,
    ) -> u64 {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        let id = inner.next_id;
        inner.next_id += 1;

        let error = AppError {
            id,
            timestamp: SystemTime::now(),
            severity,
            category,
            message: message.into(),
            details: details.map(Into::into),
            recovery_hint: Some(recovery_hint.into()),
            acknowledged: false,
        };

        if severity >= ErrorSeverity::Error {
            inner.healthy = false;
            inner.unhealthy_reason = Some(error.message.clone());
        }

        if inner.errors.len() == inner.capacity {
            inner.errors.pop_front();
        }
        inner.errors.push_back(error);

        id
    }

    /// Get the most recent critical/error (for header display).
    pub fn current_error(&self) -> Option<AppError> {
        let inner = self.inner.read().expect("error registry lock poisoned");
        inner
            .errors
            .iter()
            .rev()
            .find(|e| !e.acknowledged && e.severity >= ErrorSeverity::Warning)
            .cloned()
    }

    /// Get all errors for diagnostics panel.
    pub fn all_errors(&self) -> Vec<AppError> {
        let inner = self.inner.read().expect("error registry lock poisoned");
        inner.errors.iter().cloned().collect()
    }

    /// Get errors by category.
    pub fn errors_by_category(&self, category: ErrorCategory) -> Vec<AppError> {
        let inner = self.inner.read().expect("error registry lock poisoned");
        inner
            .errors
            .iter()
            .filter(|e| e.category == category)
            .cloned()
            .collect()
    }

    /// Acknowledge an error (removes from header display).
    pub fn acknowledge(&self, error_id: u64) {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        if let Some(error) = inner.errors.iter_mut().find(|e| e.id == error_id) {
            error.acknowledged = true;
        }
    }

    /// Acknowledge all errors.
    pub fn acknowledge_all(&self) {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        for error in inner.errors.iter_mut() {
            error.acknowledged = true;
        }
    }

    /// Clear errors older than duration.
    pub fn clear_old(&self, older_than: Duration) {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        let cutoff = SystemTime::now()
            .checked_sub(older_than)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        inner.errors.retain(|e| e.timestamp >= cutoff);
    }

    /// Start tracking a recovery operation.
    pub fn start_recovery(&self, operation: impl Into<String>, max_attempts: u32) {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        let operation = operation.into();

        // Remove existing recovery for same operation
        inner.recoveries.retain(|r| r.operation != operation);

        inner.recoveries.push(RecoveryState {
            operation,
            attempt: 1,
            max_attempts,
            next_retry: None,
            succeeded: false,
        });
    }

    /// Update recovery attempt.
    pub fn update_recovery(&self, operation: &str, attempt: u32, next_retry: Option<SystemTime>) {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        if let Some(recovery) = inner.recoveries.iter_mut().find(|r| r.operation == operation) {
            recovery.attempt = attempt;
            recovery.next_retry = next_retry;
        }
    }

    /// Mark recovery as succeeded.
    pub fn recovery_succeeded(&self, operation: &str) {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        inner.recoveries.retain(|r| r.operation != operation);

        // Restore health if no more recoveries and no unacknowledged errors
        if inner.recoveries.is_empty() {
            let has_active_errors = inner
                .errors
                .iter()
                .any(|e| !e.acknowledged && e.severity >= ErrorSeverity::Error);
            if !has_active_errors {
                inner.healthy = true;
                inner.unhealthy_reason = None;
            }
        }
    }

    /// Mark recovery as failed.
    pub fn recovery_failed(&self, operation: &str) {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        if let Some(recovery) = inner.recoveries.iter_mut().find(|r| r.operation == operation) {
            recovery.succeeded = false;
        }
        // Don't remove - let UI show failed state
    }

    /// Get current recovery operations.
    pub fn active_recoveries(&self) -> Vec<RecoveryState> {
        let inner = self.inner.read().expect("error registry lock poisoned");
        inner.recoveries.clone()
    }

    /// Check if system is healthy.
    pub fn is_healthy(&self) -> bool {
        let inner = self.inner.read().expect("error registry lock poisoned");
        inner.healthy
    }

    /// Set system health status.
    pub fn set_health(&self, healthy: bool, reason: Option<String>) {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        inner.healthy = healthy;
        inner.unhealthy_reason = reason;
    }

    /// Mark a feature as degraded.
    pub fn degrade_feature(&self, feature: Feature, reason: impl Into<String>) {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        inner.degraded_features.insert(feature);

        // Also record as warning
        let id = inner.next_id;
        inner.next_id += 1;

        let error = AppError {
            id,
            timestamp: SystemTime::now(),
            severity: ErrorSeverity::Warning,
            category: ErrorCategory::System,
            message: reason.into(),
            details: None,
            recovery_hint: None,
            acknowledged: false,
        };

        if inner.errors.len() == inner.capacity {
            inner.errors.pop_front();
        }
        inner.errors.push_back(error);
    }

    /// Check if feature is available.
    pub fn is_feature_available(&self, feature: Feature) -> bool {
        let inner = self.inner.read().expect("error registry lock poisoned");
        !inner.degraded_features.contains(&feature)
    }

    /// Restore a feature.
    pub fn restore_feature(&self, feature: Feature) {
        let mut inner = self.inner.write().expect("error registry lock poisoned");
        inner.degraded_features.remove(&feature);
    }

    /// Get list of degraded features.
    pub fn degraded_features(&self) -> Vec<Feature> {
        let inner = self.inner.read().expect("error registry lock poisoned");
        inner.degraded_features.iter().copied().collect()
    }
}

impl Default for ErrorRegistry {
    fn default() -> Self {
        Self::new(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_error() {
        let registry = ErrorRegistry::new(10);
        let id = registry.record(ErrorSeverity::Error, ErrorCategory::Network, "Connection failed");
        assert_eq!(id, 1);

        let error = registry.current_error().unwrap();
        assert_eq!(error.id, 1);
        assert_eq!(error.message, "Connection failed");
        assert_eq!(error.severity, ErrorSeverity::Error);
    }

    #[test]
    fn test_acknowledge_error() {
        let registry = ErrorRegistry::new(10);
        let id = registry.record(ErrorSeverity::Error, ErrorCategory::Network, "Error");

        assert!(registry.current_error().is_some());
        registry.acknowledge(id);
        assert!(registry.current_error().is_none());
    }

    #[test]
    fn test_recovery_tracking() {
        let registry = ErrorRegistry::new(10);

        registry.start_recovery("backend_connection", 3);
        let recoveries = registry.active_recoveries();
        assert_eq!(recoveries.len(), 1);
        assert_eq!(recoveries[0].operation, "backend_connection");
        assert_eq!(recoveries[0].attempt, 1);

        registry.update_recovery("backend_connection", 2, None);
        let recoveries = registry.active_recoveries();
        assert_eq!(recoveries[0].attempt, 2);

        registry.recovery_succeeded("backend_connection");
        assert!(registry.active_recoveries().is_empty());
    }

    #[test]
    fn test_feature_degradation() {
        let registry = ErrorRegistry::new(10);

        assert!(registry.is_feature_available(Feature::Clipboard));
        registry.degrade_feature(Feature::Clipboard, "Headless mode");
        assert!(!registry.is_feature_available(Feature::Clipboard));

        registry.restore_feature(Feature::Clipboard);
        assert!(registry.is_feature_available(Feature::Clipboard));
    }

    #[test]
    fn test_ring_buffer() {
        let registry = ErrorRegistry::new(3);

        registry.record(ErrorSeverity::Info, ErrorCategory::System, "Error 1");
        registry.record(ErrorSeverity::Info, ErrorCategory::System, "Error 2");
        registry.record(ErrorSeverity::Info, ErrorCategory::System, "Error 3");
        registry.record(ErrorSeverity::Info, ErrorCategory::System, "Error 4");

        let errors = registry.all_errors();
        assert_eq!(errors.len(), 3);
        assert_eq!(errors[0].message, "Error 2");
        assert_eq!(errors[2].message, "Error 4");
    }

    #[test]
    fn test_health_status() {
        let registry = ErrorRegistry::new(10);
        assert!(registry.is_healthy());

        registry.record(ErrorSeverity::Error, ErrorCategory::Network, "Failed");
        assert!(!registry.is_healthy());

        registry.set_health(true, None);
        assert!(registry.is_healthy());
    }
}
