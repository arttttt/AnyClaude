use anyclaude::proxy::shutdown::ShutdownManager;
use anyclaude::shutdown::{ShutdownCoordinator, ShutdownPhase};
use std::time::Duration;

// Tests for ShutdownCoordinator
#[test]
fn test_shutdown_coordinator_initialization() {
    let coordinator = ShutdownCoordinator::new();
    assert!(!coordinator.is_shutting_down());
    assert_eq!(coordinator.phase(), ShutdownPhase::Running);
}

#[test]
fn test_shutdown_coordinator_signal() {
    let coordinator = ShutdownCoordinator::new();
    assert!(!coordinator.is_shutting_down());

    coordinator.signal();
    assert!(coordinator.is_shutting_down());

    // Signal again should be idempotent
    coordinator.signal();
    assert!(coordinator.is_shutting_down());
}

#[test]
fn test_shutdown_coordinator_phases() {
    let coordinator = ShutdownCoordinator::new();

    coordinator.advance(ShutdownPhase::Signaled);
    assert_eq!(coordinator.phase(), ShutdownPhase::Signaled);

    coordinator.advance(ShutdownPhase::StoppingInput);
    assert_eq!(coordinator.phase(), ShutdownPhase::StoppingInput);

    coordinator.advance(ShutdownPhase::Complete);
    assert_eq!(coordinator.phase(), ShutdownPhase::Complete);
}

#[test]
fn test_shutdown_handle_shares_state() {
    let coordinator = ShutdownCoordinator::new();
    let handle = coordinator.handle();

    assert!(!handle.is_shutting_down());

    coordinator.signal();
    assert!(handle.is_shutting_down());
}

#[test]
fn test_shutdown_handle_can_signal() {
    let coordinator = ShutdownCoordinator::new();
    let handle = coordinator.handle();

    handle.signal();
    assert!(coordinator.is_shutting_down());
    assert!(handle.is_shutting_down());
}

// Tests for proxy ShutdownManager
#[tokio::test]
async fn test_shutdown_manager_initialization() {
    let manager = ShutdownManager::new();
    assert!(!manager.is_shutting_down());
}

#[tokio::test]
async fn test_wait_for_connections_completes_immediately_when_zero() {
    let manager = ShutdownManager::new();
    
    let start = std::time::Instant::now();
    manager.wait_for_connections(Duration::from_secs(1)).await;
    let elapsed = start.elapsed();
    
    assert!(elapsed < Duration::from_millis(100));
}

#[tokio::test]
async fn test_wait_for_connections_times_out() {
    let manager = ShutdownManager::new();
    manager.increment_connections();
    
    let start = std::time::Instant::now();
    manager.wait_for_connections(Duration::from_millis(100)).await;
    let elapsed = start.elapsed();
    
    assert!(elapsed >= Duration::from_millis(90));
}
