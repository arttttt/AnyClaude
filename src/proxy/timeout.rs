//! Timeout configuration and utilities for proxy requests.
//!
//! Provides timeout settings for connection establishment,
//! total request duration, and idle streaming timeouts.

use crate::config::Defaults;
use std::time::Duration;

/// Timeout configuration for proxy requests
#[derive(Debug, Clone, Copy)]
pub struct TimeoutConfig {
    /// Time to establish TCP connection
    pub connect: Duration,
    /// Total time for complete request/response
    pub request: Duration,
    /// Max time between bytes for streaming responses
    pub idle: Duration,
}

impl TimeoutConfig {
    /// Create a new timeout configuration with explicit values
    pub fn new(connect_secs: u64, request_secs: u64, idle_secs: u64) -> Self {
        Self {
            connect: Duration::from_secs(connect_secs),
            request: Duration::from_secs(request_secs),
            idle: Duration::from_secs(idle_secs),
        }
    }
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            request: Duration::from_secs(30),
            idle: Duration::from_secs(60),
        }
    }
}

impl From<&Defaults> for TimeoutConfig {
    fn from(defaults: &Defaults) -> Self {
        Self {
            connect: Duration::from_secs(defaults.connect_timeout_seconds.into()),
            request: Duration::from_secs(defaults.timeout_seconds.into()),
            idle: Duration::from_secs(defaults.idle_timeout_seconds.into()),
        }
    }
}