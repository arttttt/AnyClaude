use claudewrapper::proxy::shutdown::ShutdownManager;
use std::time::Duration;

#[tokio::test]
async fn test_shutdown_manager_initialization() {
    let manager = ShutdownManager::new();
    assert!(!manager.is_shutting_down());
}

#[test]
fn test_connection_tracking() {
    let manager = ShutdownManager::new();
    
    manager.increment_connections();
    manager.increment_connections();
    manager.increment_connections();
    
    manager.decrement_connections();
    manager.increment_connections();
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
