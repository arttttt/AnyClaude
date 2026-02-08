use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use crate::metrics::{app_log};
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ShutdownPhase {
    Running = 0,
    Signaled = 1,
    StoppingInput = 2,
    TerminatingChild = 3,
    ClosingProxy = 4,
    Cleanup = 5,
    Complete = 6,
}

pub struct ShutdownCoordinator {
    shutdown: Arc<AtomicBool>,
    phase: Arc<AtomicU8>,
    notify: Arc<Notify>,
}

impl ShutdownCoordinator {
    pub fn new() -> Self {
        Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            phase: Arc::new(AtomicU8::new(ShutdownPhase::Running as u8)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Signal shutdown start
    pub fn signal(&self) {
        if !self.shutdown.swap(true, Ordering::SeqCst) {
            app_log("shutdown", "Graceful shutdown initiated");
            self.notify.notify_waiters();
        }
    }

    /// Check if shutdown is in progress
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Get current phase
    pub fn phase(&self) -> ShutdownPhase {
        match self.phase.load(Ordering::SeqCst) {
            0 => ShutdownPhase::Running,
            1 => ShutdownPhase::Signaled,
            2 => ShutdownPhase::StoppingInput,
            3 => ShutdownPhase::TerminatingChild,
            4 => ShutdownPhase::ClosingProxy,
            5 => ShutdownPhase::Cleanup,
            _ => ShutdownPhase::Complete,
        }
    }

    /// Advance to next phase
    pub fn advance(&self, phase: ShutdownPhase) {
        self.phase.store(phase as u8, Ordering::SeqCst);
        app_log("shutdown", &format!("Shutdown phase: {:?}", phase));
    }

    /// Create a handle for sharing
    pub fn handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            shutdown: Arc::clone(&self.shutdown),
            notify: Arc::clone(&self.notify),
        }
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// Lightweight handle for checking shutdown state
#[derive(Clone)]
pub struct ShutdownHandle {
    shutdown: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl ShutdownHandle {
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    pub fn signal(&self) {
        if !self.shutdown.swap(true, Ordering::SeqCst) {
            self.notify.notify_waiters();
        }
    }

    pub async fn wait(&self) {
        // Subscribe to Notify BEFORE checking the flag to avoid TOCTOU race:
        // without this, signal() could fire between the check and the await,
        // and notify_waiters() would have no subscribers, losing the notification.
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_shutting_down() {
            return;
        }
        notified.await;
    }
}
