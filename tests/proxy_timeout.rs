mod common;

use std::time::Duration;

use anyclaude::config::Defaults;
use anyclaude::proxy::timeout::TimeoutConfig;

#[test]
fn test_default_timeouts() {
    let config = TimeoutConfig::default();
    assert_eq!(config.connect, Duration::from_secs(5));
    assert_eq!(config.request, Duration::from_secs(30));
    assert_eq!(config.idle, Duration::from_secs(60));
}

#[test]
fn test_custom_timeouts() {
    let config = TimeoutConfig::new(10, 60, 120);
    assert_eq!(config.connect, Duration::from_secs(10));
    assert_eq!(config.request, Duration::from_secs(60));
    assert_eq!(config.idle, Duration::from_secs(120));
}

#[test]
fn test_from_defaults() {
    let defaults = Defaults {
        active: "test".to_string(),
        timeout_seconds: 45,
        connect_timeout_seconds: 10,
        idle_timeout_seconds: 90,
        pool_idle_timeout_seconds: 120,
        pool_max_idle_per_host: 4,
        max_retries: 2,
        retry_backoff_base_ms: 150,
    };

    let config = TimeoutConfig::from(&defaults);
    assert_eq!(config.request, Duration::from_secs(45));
    assert_eq!(config.connect, Duration::from_secs(10));
    assert_eq!(config.idle, Duration::from_secs(90));
}
