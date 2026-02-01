use crossterm::event::{self, Event, KeyEvent};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use crate::ipc::{BackendInfo, ProxyStatus};
use crate::metrics::MetricsSnapshot;
use crate::shutdown::ShutdownHandle;

/// Error types for PTY operations.
#[derive(Debug, Clone)]
pub enum PtyError {
    /// Child process exited with code
    ProcessExited { exit_code: Option<i32> },
    /// Spawn failed
    SpawnFailed { command: String, error: String },
    /// Read error from PTY
    ReadError { error: String },
}

impl PtyError {
    /// User-friendly message for display.
    pub fn user_message(&self) -> &'static str {
        match self {
            PtyError::ProcessExited { .. } => "Claude Code has exited",
            PtyError::SpawnFailed { .. } => "Failed to start Claude Code",
            PtyError::ReadError { .. } => "Lost connection to Claude Code",
        }
    }

    /// Technical details for diagnostics.
    pub fn details(&self) -> String {
        match self {
            PtyError::ProcessExited { exit_code } => match exit_code {
                Some(code) => format!("Process exited with code {}", code),
                None => "Process exited (unknown code)".to_string(),
            },
            PtyError::SpawnFailed { command, error } => {
                format!("Failed to spawn '{}': {}", command, error)
            }
            PtyError::ReadError { error } => format!("PTY read error: {}", error),
        }
    }
}

pub enum AppEvent {
    Input(KeyEvent),
    Paste(String),
    /// Image paste: data URI
    ImagePaste(String),
    Tick,
    Resize(u16, u16),
    PtyOutput,
    /// Config file was successfully reloaded
    ConfigReload,
    /// Config reload failed
    ConfigError(String),
    IpcStatus(ProxyStatus),
    IpcMetrics(MetricsSnapshot),
    IpcBackends(Vec<BackendInfo>),
    IpcError(String),
    /// PTY error occurred
    PtyError(PtyError),
    /// OS signal received (SIGTERM, SIGINT)
    Shutdown,
    /// Claude child process exited (EOF from PTY reader)
    ProcessExit,
}

pub struct EventHandler {
    rx: Receiver<AppEvent>,
    tx: mpsc::Sender<AppEvent>,
}

impl EventHandler {
    pub fn new(tick_rate: Duration, shutdown: ShutdownHandle) -> Self {
        let (tx, rx) = mpsc::channel();
        let event_tx = tx.clone();

        thread::spawn(move || {
            let mut last_tick = Instant::now();
            loop {
                // Check shutdown flag before blocking on poll
                if shutdown.is_shutting_down() {
                    break;
                }

                // Use short poll timeout to check shutdown flag frequently
                let timeout = tick_rate.saturating_sub(last_tick.elapsed()).min(Duration::from_millis(50));
                if event::poll(timeout).unwrap_or(false) {
                    match event::read() {
                        Ok(Event::Key(key)) => {
                            let _ = event_tx.send(AppEvent::Input(key));
                        }
                        Ok(Event::Paste(text)) => {
                            let _ = event_tx.send(AppEvent::Paste(text));
                        }
                        Ok(Event::Resize(cols, rows)) => {
                            let _ = event_tx.send(AppEvent::Resize(cols, rows));
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }

                if last_tick.elapsed() >= tick_rate {
                    let _ = event_tx.send(AppEvent::Tick);
                    last_tick = Instant::now();
                }
            }
        });

        Self { rx, tx }
    }

    pub fn next(&self, timeout: Duration) -> Result<AppEvent, mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }

    pub fn sender(&self) -> mpsc::Sender<AppEvent> {
        self.tx.clone()
    }
}
