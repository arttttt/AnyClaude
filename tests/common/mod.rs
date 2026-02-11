//! Shared test utilities and mock infrastructure.

#![allow(dead_code, unused_imports)]

pub mod mock_backend;

use anyclaude::config::{Config, ConfigStore};
use anyclaude::pty::emulator::TerminalEmulator;
use anyclaude::pty::PtyHandle;
use anyclaude::ui::app::App;
use parking_lot::Mutex;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

pub type SharedEmulator = Arc<Mutex<Box<dyn TerminalEmulator>>>;
pub type SpyBuffer = Arc<Mutex<Vec<u8>>>;

/// Find an available port for testing.
pub fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind to free port");
    listener.local_addr().unwrap().port()
}

/// Create a temporary config file with specified backends.
pub fn temp_config(backends: &[(&str, &str, &str)]) -> (TempDir, PathBuf) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config_path = temp_dir.path().join("config.toml");

    let mut content = String::from(
        r#"[defaults]
active = "test"
timeout_seconds = 5
connect_timeout_seconds = 2

[proxy]
bind_addr = "127.0.0.1:0"

"#,
    );

    for (name, url, auth_type) in backends {
        content.push_str(&format!(
            r#"[[backends]]
name = "{}"
display_name = "{}"
base_url = "{}"
auth_type = "{}"
"#,
            name,
            name.to_uppercase(),
            url,
            auth_type
        ));
        if *auth_type == "api_key" {
            content.push_str("api_key = \"test-key\"\n");
        }
        content.push('\n');
    }

    std::fs::write(&config_path, content).expect("Failed to write config");
    (temp_dir, config_path)
}

/// Wait for a server to become available.
pub async fn wait_for_server(addr: SocketAddr, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

// -- App helpers --------------------------------------------------------------

pub fn make_app() -> App {
    let config = ConfigStore::new(Config::default(), PathBuf::from("/tmp/test.toml"));
    App::new(config)
}


// -- PTY mocks ----------------------------------------------------------------

/// Writer that records all bytes sent through `PtyHandle::send_input`.
pub struct SpyWriter(Arc<Mutex<Vec<u8>>>);

impl SpyWriter {
    pub fn new(buf: Arc<Mutex<Vec<u8>>>) -> Self {
        Self(buf)
    }
}

impl std::io::Write for SpyWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Minimal stub satisfying the `MasterPty` trait.
pub struct MockMasterPty;

impl portable_pty::MasterPty for MockMasterPty {
    fn resize(&self, _: portable_pty::PtySize) -> anyhow::Result<()> {
        Ok(())
    }
    fn get_size(&self) -> anyhow::Result<portable_pty::PtySize> {
        Ok(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
    }
    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn std::io::Read + Send>> {
        Ok(Box::new(std::io::empty()))
    }
    fn take_writer(&self) -> anyhow::Result<Box<dyn std::io::Write + Send>> {
        Ok(Box::new(std::io::sink()))
    }
    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<libc::pid_t> {
        None
    }
    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<std::os::unix::io::RawFd> {
        None
    }
    #[cfg(unix)]
    fn tty_name(&self) -> Option<std::path::PathBuf> {
        None
    }
}

// -- Composite builders -------------------------------------------------------

/// Build an `App` wired to a real terminal emulator and spy writer.
///
/// Returns `(app, spy_buffer, emulator)`.
pub fn make_app_with_pty() -> (App, SpyBuffer, SharedEmulator) {
    let mut app = make_app();
    let emu: SharedEmulator =
        Arc::new(Mutex::new(anyclaude::pty::emulator::create(24, 80, 0)));
    let spy_buf: SpyBuffer = Arc::new(Mutex::new(Vec::new()));
    let writer: Box<dyn std::io::Write + Send> = Box::new(SpyWriter::new(spy_buf.clone()));
    let master: Box<dyn portable_pty::MasterPty + Send> = Box::new(MockMasterPty);
    let handle = PtyHandle::new(emu.clone(), writer, Arc::new(Mutex::new(master)));
    app.attach_pty(handle);
    (app, spy_buf, emu)
}
