use parking_lot::Mutex;
use portable_pty::{MasterPty, PtySize};
use std::error::Error;
use std::sync::Arc;
use std::thread;

#[cfg(unix)]
use crossterm::terminal::size as terminal_size;
#[cfg(unix)]
use signal_hook::consts::signal::SIGWINCH;
#[cfg(unix)]
use signal_hook::iterator::Signals;

pub struct ResizeWatcher {
    #[cfg(unix)]
    handle: signal_hook::iterator::Handle,
    #[cfg(unix)]
    thread: thread::JoinHandle<()>,
}

impl ResizeWatcher {
    pub fn start(
        master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
        parser: Arc<Mutex<vt100::Parser>>,
    ) -> Result<Option<Self>, Box<dyn Error>> {
        #[cfg(unix)]
        {
            let mut signals = Signals::new([SIGWINCH])?;
            let handle = signals.handle();
            let thread = thread::spawn(move || {
                for _ in signals.forever() {
                    let (cols, rows) = match terminal_size() {
                        Ok(size) => size,
                        Err(_) => continue,
                    };
                    let size = PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    };
                    let _ = master.lock().resize(size);
                    parser.lock().screen_mut().set_size(rows, cols);
                }
            });
            return Ok(Some(Self { handle, thread }));
        }

        #[cfg(not(unix))]
        {
            let _ = master;
            let _ = parser;
            Ok(None)
        }
    }

    pub fn stop(self) {
        #[cfg(unix)]
        {
            self.handle.close();
            let _ = self.thread.join();
        }
    }
}
