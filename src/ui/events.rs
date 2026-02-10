use crossterm::event::{self, Event, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
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
    Mouse(MouseEvent),
    Paste(String),
    /// Image paste: path to saved temp file
    ImagePaste(std::path::PathBuf),
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
                        Ok(Event::Mouse(mouse)) => {
                            let _ = event_tx.send(AppEvent::Mouse(mouse));
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

/// Extract scroll direction from mouse event.
/// Returns Some((up, lines)) where up=true means scroll up (view older content).
pub fn mouse_scroll_direction(event: &MouseEvent) -> Option<(bool, usize)> {
    match event.kind {
        MouseEventKind::ScrollUp => Some((true, 3)),
        MouseEventKind::ScrollDown => Some((false, 3)),
        _ => None,
    }
}

/// Convert mouse event to xterm mouse protocol bytes.
/// Returns Some(bytes) if the event should be sent to the PTY.
///
/// X10 mouse protocol format:
/// - ESC [ M followed by 3 bytes:
///   - byte 1: button + 32 (0b00=left, 0b01=middle, 0b10=right, 0b11=release)
///   - byte 2: column + 33
///   - byte 3: row + 33
pub fn mouse_event_to_pty_bytes(event: &MouseEvent) -> Option<Vec<u8>> {
    // Only handle press, release, and drag events (scroll is handled separately)
    // Skip Moved events (motion without buttons) to avoid garbage in input
    let button_code = match event.kind {
        MouseEventKind::Down(button) => match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        },
        MouseEventKind::Up(_) => 3, // Button release
        MouseEventKind::Drag(button) => match button {
            MouseButton::Left => 32,
            MouseButton::Middle => 33,
            MouseButton::Right => 34,
        },
        _ => return None, // Scroll and Moved handled separately/not sent
    };

    // X10 encoding: add 32 to button code to make it printable
    // add 33 to coordinates (1-based)
    let encoded_button = (button_code + 32) as u8;
    let encoded_col = (event.column + 33) as u8;
    let encoded_row = (event.row + 33) as u8;

    Some(vec![0x1b, b'[', b'M', encoded_button, encoded_col, encoded_row])
}
