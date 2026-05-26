//! Lightweight shell PTY session for the GPU UI.
//!
//! Spawns the user's shell via `portable-pty` and ships its output
//! bytes through an `mpsc::channel`. A user-supplied callback fires
//! after every successful read so the host event loop (winit) can wake
//! up and drain. Resize and write are direct pass-throughs to the
//! master PTY.
//!
//! This is intentionally simpler than the legacy `pty::PtySession` —
//! no IPC, no shutdown coordinator, no proxy. The full bootstrap is
//! wired in at the C10 cutover.

use std::io::{self, Read, Write};
use std::sync::mpsc;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

pub struct ShellPty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    bytes_rx: mpsc::Receiver<Vec<u8>>,
}

impl ShellPty {
    /// Spawn the user's `$SHELL` (or `/bin/sh` fallback) at the given
    /// grid size. `on_data` is invoked from the reader thread after
    /// every successful read so the caller can request a redraw.
    pub fn spawn<F>(cols: u16, rows: u16, on_data: F) -> io::Result<Self>
    where
        F: Fn() + Send + 'static,
    {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::other(e.to_string()))?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let cmd = CommandBuilder::new(shell);
        pair.slave
            .spawn_command(cmd)
            .map_err(|e| io::Error::other(e.to_string()))?;
        // Drop slave so the PTY closes when the shell exits.
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                        on_data();
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            master: pair.master,
            writer,
            bytes_rx: rx,
        })
    }

    /// Drain every byte chunk currently queued by the reader thread.
    /// Returns empty when no PTY output is pending.
    pub fn drain(&mut self) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        while let Ok(chunk) = self.bytes_rx.try_recv() {
            chunks.push(chunk);
        }
        chunks
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// Write `bytes` to the PTY's stdin. Returns an error when the
    /// shell has closed (broken pipe) or the underlying write fails.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }
}
