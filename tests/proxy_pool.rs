mod common;

use std::time::Duration;

use anyclaude::proxy::pool::PoolConfig;

#[test]
fn test_default_pool_config() {
    let config = PoolConfig::default();
    assert_eq!(config.pool_idle_timeout, Duration::from_secs(90));
    assert_eq!(config.pool_max_idle_per_host, 8);
    assert_eq!(config.max_retries, 3);
    assert_eq!(config.retry_backoff_base, Duration::from_millis(100));
}

#[test]
fn test_custom_pool_config() {
    let config = PoolConfig::new(10, 2, 5, 250);
    assert_eq!(config.pool_idle_timeout, Duration::from_secs(10));
    assert_eq!(config.pool_max_idle_per_host, 2);
    assert_eq!(config.max_retries, 5);
    assert_eq!(config.retry_backoff_base, Duration::from_millis(250));
}
