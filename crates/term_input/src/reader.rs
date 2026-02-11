use crate::event::InputEvent;
use crate::parser::InputParser;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::time::Duration;

/// ESC timeout in milliseconds.
/// If ESC is received and no follow-up byte arrives within this time,
/// the ESC is emitted as a bare Escape key.
const ESC_TIMEOUT_MS: i32 = 2;

/// Terminal input reader — opens /dev/tty and reads parsed input events.
///
/// Uses `std::fs::File` for I/O and `select()` for readiness polling
/// (more reliable than `poll()` on macOS).
pub struct TtyReader {
    file: std::fs::File,
    parser: InputParser,
    pending: VecDeque<InputEvent>,
    buf: [u8; 1024],
}

impl TtyReader {
    /// Open the terminal for reading.
    ///
    /// Prefers stdin (if it's a tty) — matches how Node.js/Ink reads input,
    /// important for Warp terminal compatibility where /dev/tty may behave
    /// differently from stdin for Option/Alt key handling.
    /// Falls back to `/dev/tty` if stdin is not a terminal.
    pub fn open() -> io::Result<Self> {
        let file = Self::dup_stdin_if_tty().or_else(|_| Self::open_tty())?;
        // Set close-on-exec to prevent leaking fd to child processes
        unsafe {
            libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        }
        Ok(Self {
            file,
            parser: InputParser::new(),
            pending: VecDeque::new(),
            buf: [0u8; 1024],
        })
    }

    fn open_tty() -> io::Result<std::fs::File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
    }

    fn dup_stdin_if_tty() -> io::Result<std::fs::File> {
        let is_tty = unsafe { libc::isatty(libc::STDIN_FILENO) == 1 };
        if !is_tty {
            return Err(io::Error::new(io::ErrorKind::NotFound, "stdin is not a tty"));
        }
        let fd = unsafe { libc::dup(libc::STDIN_FILENO) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }

    /// Read the next input event, blocking up to `timeout`.
    ///
    /// Returns `Ok(None)` on timeout (no complete event available).
    /// Handles ESC timeout internally: if ESC was the last byte and no
    /// follow-up arrives within 2ms, emits it as a bare Escape key.
    pub fn read(&mut self, timeout: Duration) -> io::Result<Option<InputEvent>> {
        // Return buffered events first
        if let Some(event) = self.pending.pop_front() {
            return Ok(Some(event));
        }

        // Poll for data
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        if !self.select(timeout_ms)? {
            // Timeout — check if parser has pending ESC
            if self.parser.has_pending() {
                let events = self.parser.flush();
                self.pending.extend(events);
                return Ok(self.pending.pop_front());
            }
            return Ok(None);
        }

        // Read available bytes (std::io::Read retries on EINTR automatically)
        let n = self.file.read(&mut self.buf)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "tty closed"));
        }

        // Feed to parser
        let events = self.parser.feed(&self.buf[..n]);
        self.pending.extend(events);

        // Handle ESC timeout: if parser has pending state, do a short poll
        if self.parser.has_pending() {
            if !self.select(ESC_TIMEOUT_MS)? {
                // No more data within ESC timeout — flush
                let events = self.parser.flush();
                self.pending.extend(events);
            } else {
                // More data available — read and feed
                let n = self.file.read(&mut self.buf)?;
                if n > 0 {
                    let events = self.parser.feed(&self.buf[..n]);
                    self.pending.extend(events);
                }
            }
        }

        Ok(self.pending.pop_front())
    }

    /// Raw file descriptor, for external polling if needed.
    pub fn fd(&self) -> std::os::unix::io::RawFd {
        self.file.as_raw_fd()
    }

    /// Wait for data using `select()` — reliable on macOS (unlike `poll()`).
    fn select(&self, timeout_ms: i32) -> io::Result<bool> {
        let fd = self.file.as_raw_fd();
        unsafe {
            let mut read_fds: libc::fd_set = std::mem::zeroed();
            libc::FD_ZERO(&mut read_fds);
            libc::FD_SET(fd, &mut read_fds);

            let mut tv = libc::timeval {
                tv_sec: (timeout_ms / 1000) as libc::time_t,
                tv_usec: ((timeout_ms % 1000) * 1000) as libc::suseconds_t,
            };

            loop {
                let ret = libc::select(
                    fd + 1,
                    &mut read_fds,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut tv,
                );
                if ret < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::Interrupted {
                        // EINTR: re-init fd_set (select may have clobbered it)
                        libc::FD_ZERO(&mut read_fds);
                        libc::FD_SET(fd, &mut read_fds);
                        continue;
                    }
                    return Err(err);
                }
                return Ok(ret > 0);
            }
        }
    }
}
