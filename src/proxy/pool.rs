//! Connection pool and retry configuration for upstream requests.

use std::time::Duration;

use crate::config::Defaults;

/// Pool and retry configuration for upstream requests.
#[derive(Debug, Clone, Copy)]
pub struct PoolConfig {
    /// Idle timeout for pooled connections.
    pub pool_idle_timeout: Duration,
    /// Max idle connections per host.
    pub pool_max_idle_per_host: usize,
    /// Max retry attempts for connection errors.
    pub max_retries: u32,
    /// Base backoff duration for retries.
    pub retry_backoff_base: Duration,
}

impl PoolConfig {
    /// Create a new pool configuration with explicit values.
    pub fn new(
        pool_idle_timeout_secs: u64,
        pool_max_idle_per_host: usize,
        max_retries: u32,
        retry_backoff_base_ms: u64,
    ) -> Self {
        Self {
            pool_idle_timeout: Duration::from_secs(pool_idle_timeout_secs),
            pool_max_idle_per_host,
            max_retries,
            retry_backoff_base: Duration::from_millis(retry_backoff_base_ms),
        }
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            pool_idle_timeout: Duration::from_secs(90),
            pool_max_idle_per_host: 8,
            max_retries: 3,
            retry_backoff_base: Duration::from_millis(100),
        }
    }
}

impl From<&Defaults> for PoolConfig {
    fn from(defaults: &Defaults) -> Self {
        Self {
            pool_idle_timeout: Duration::from_secs(defaults.pool_idle_timeout_seconds.into()),
            pool_max_idle_per_host: defaults.pool_max_idle_per_host as usize,
            max_retries: defaults.max_retries,
            retry_backoff_base: Duration::from_millis(defaults.retry_backoff_base_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
