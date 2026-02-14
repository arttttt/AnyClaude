use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use term_input::{InputEvent, TtyReader};

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
    Key(term_input::KeyInput),
    Mouse(term_input::MouseEvent),
    Paste(String),
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
    /// Claude child process exited (EOF from PTY reader).
    /// Tagged with PTY generation to ignore stale exits from old instances.
    ProcessExit { pty_generation: u64 },
    /// PTY restart requested (settings changed)
    PtyRestart {
        env_vars: Vec<(String, String)>,
        cli_args: Vec<String>,
    },
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
            let mut reader = match TtyReader::open() {
                Ok(r) => r,
                Err(err) => {
                    crate::metrics::app_log_error(
                        "events",
                        "Failed to open /dev/tty",
                        &err.to_string(),
                    );
                    return;
                }
            };

            // Register SIGWINCH handler to detect terminal resize
            let sigwinch_flag = Arc::new(AtomicBool::new(false));
            let _ = signal_hook::flag::register(libc::SIGWINCH, Arc::clone(&sigwinch_flag));

            let mut last_tick = Instant::now();
            loop {
                if shutdown.is_shutting_down() {
                    break;
                }

                // Check for SIGWINCH (terminal resize)
                if sigwinch_flag.swap(false, Ordering::Relaxed) {
                    if let Some((cols, rows)) = query_terminal_size() {
                        let _ = event_tx.send(AppEvent::Resize(cols, rows));
                    }
                }

                // Use short poll timeout to check shutdown flag frequently
                let timeout =
                    tick_rate.saturating_sub(last_tick.elapsed()).min(Duration::from_millis(50));

                match reader.read(timeout) {
                    Ok(Some(InputEvent::Key(key))) => {
                        let _ = event_tx.send(AppEvent::Key(key));
                    }
                    Ok(Some(InputEvent::Mouse(mouse))) => {
                        let _ = event_tx.send(AppEvent::Mouse(mouse));
                    }
                    Ok(Some(InputEvent::Paste(text))) => {
                        let _ = event_tx.send(AppEvent::Paste(text));
                    }
                    Ok(None) => {
                        // Timeout — no event
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(err) => {
                        crate::metrics::app_log_error(
                            "events",
                            "TtyReader error",
                            &err.to_string(),
                        );
                        break;
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

/// Query terminal size using ioctl TIOCGWINSZ.
fn query_terminal_size() -> Option<(u16, u16)> {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_col > 0
            && ws.ws_row > 0
        {
            Some((ws.ws_col, ws.ws_row))
        } else {
            None
        }
    }
}
