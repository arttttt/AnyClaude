use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::Notify;

use crate::metrics::app_log;

pub struct ShutdownManager {
    shutdown: Arc<AtomicBool>,
    active_connections: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

impl ShutdownManager {
    pub fn new() -> Self {
        Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            active_connections: Arc::new(AtomicUsize::new(0)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub async fn wait_for_shutdown(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.is_shutting_down() {
            return Ok(());
        }

        #[cfg(unix)]
        {
            let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())?;
            tokio::select! {
                _ = signal::ctrl_c() => {},
                _ = sigterm.recv() => {},
                _ = self.notify.notified() => {},
            }
        }

        #[cfg(not(unix))]
        {
            tokio::select! {
                _ = signal::ctrl_c() => {},
                _ = self.notify.notified() => {},
            }
        }

        self.shutdown.store(true, Ordering::SeqCst);
        app_log("proxy-shutdown", "Shutting down gracefully...");
        Ok(())
    }

    pub fn signal_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    pub fn increment_connections(&self) {
        self.active_connections.fetch_add(1, Ordering::SeqCst);
    }

    pub fn decrement_connections(&self) {
        self.active_connections.fetch_sub(1, Ordering::SeqCst);
    }

    pub async fn wait_for_connections(&self, timeout: Duration) {
        let active = self.active_connections.load(Ordering::SeqCst);
        app_log("proxy-shutdown", &format!("Waiting for {} active connections...", active));

        let start = tokio::time::Instant::now();

        while start.elapsed() < timeout {
            let active = self.active_connections.load(Ordering::SeqCst);
            if active == 0 {
                app_log("proxy-shutdown", "Server stopped");
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let active = self.active_connections.load(Ordering::SeqCst);
        app_log("proxy-shutdown", &format!("Forced shutdown after timeout ({} connections remain)", active));
    }
}

impl Default for ShutdownManager {
    fn default() -> Self {
        Self::new()
    }
}
