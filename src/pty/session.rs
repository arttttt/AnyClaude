use crate::pty::handle::PtyHandle;
use crate::pty::screen::{apply_actions, ActionTranslator};
use crate::pty::vt::VtParser;
use crate::ui::events::AppEvent;
use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use std::error::Error;
use std::io::Read;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use termwiz::surface::Surface;

pub struct PtySession {
    handle: PtyHandle,
    child: Box<dyn Child + Send + Sync>,
    reader_handle: Option<thread::JoinHandle<()>>,
}

impl PtySession {
    pub fn spawn(
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
        notifier: Sender<AppEvent>,
    ) -> Result<Self, Box<dyn Error>> {
        let pty_system = native_pty_system();
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let screen = Arc::new(Mutex::new(Surface::new(
            usize::from(cols),
            usize::from(rows),
        )));

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);
        cmd.cwd(std::env::current_dir()?);
        cmd.env("TERM", "xterm-256color");
        for (key, value) in env {
            cmd.env(key, value);
        }

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let master = Arc::new(Mutex::new(pair.master));
        let handle = PtyHandle::new(Arc::clone(&screen), writer, master);

        let reader_handle = thread::spawn(move || {
            let mut reader = reader;
            let mut parser = VtParser::new();
            let mut translator = ActionTranslator::new();
            let mut buffer = [0u8; 8192];

            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => count,
                    Err(_) => break,
                };
                let actions = parser.parse(&buffer[..count]);
                if let Ok(mut screen) = screen.lock() {
                    apply_actions(&mut screen, &mut translator, actions);
                }
                let _ = notifier.send(AppEvent::PtyOutput);
            }
        });

        Ok(Self {
            handle,
            child,
            reader_handle: Some(reader_handle),
        })
    }

    pub fn handle(&self) -> PtyHandle {
        self.handle.clone()
    }

    pub fn shutdown(&mut self) -> Result<(), Box<dyn Error>> {
        // Close stdin to signal EOF to child
        self.handle.close_writer();

        // Give child a chance to exit gracefully with SIGTERM
        #[cfg(unix)]
        if let Some(pid) = self.child.process_id() {
            // SAFETY: kill() is safe to call with valid pid
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }

        // Wait with timeout for graceful exit
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        loop {
            match self.child.try_wait()? {
                Some(_) => break,
                None if std::time::Instant::now() >= deadline => {
                    // Force kill after timeout
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
                None => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }

        // Join reader thread
        if let Some(reader_handle) = self.reader_handle.take() {
            let _ = reader_handle.join();
        }
        Ok(())
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
