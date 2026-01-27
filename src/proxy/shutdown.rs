use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;

pub struct ShutdownManager {
    shutdown: Arc<AtomicBool>,
    active_connections: Arc<AtomicUsize>,
}

impl ShutdownManager {
    pub fn new() -> Self {
        Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            active_connections: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub async fn wait_for_signal(&self) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(unix)]
        {
            let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())?;
            tokio::select! {
                _ = signal::ctrl_c() => {},
                _ = sigterm.recv() => {},
            }
        }

        #[cfg(not(unix))]
        {
            signal::ctrl_c().await?;
        }

        self.shutdown.store(true, Ordering::SeqCst);
        println!("Shutting down gracefully...");
        Ok(())
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
        println!("Waiting for {} active connections...", active);

        let start = tokio::time::Instant::now();

        while start.elapsed() < timeout {
            let active = self.active_connections.load(Ordering::SeqCst);
            if active == 0 {
                println!("Server stopped");
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let active = self.active_connections.load(Ordering::SeqCst);
        println!("Forced shutdown after timeout ({} connections remain)", active);
    }
}

impl Default for ShutdownManager {
    fn default() -> Self {
        Self::new()
    }
}
