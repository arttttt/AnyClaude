mod common;

use anyclaude::error::{ErrorCategory, ErrorRegistry, ErrorSeverity, Feature};

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
