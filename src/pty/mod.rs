pub mod vt;

use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size as terminal_size};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::error::Error;
use std::io::{self, Read, Write};
use std::thread;
use termwiz::escape::Action;
use vt::VtParser;

pub struct PtyManager {
    vt_parser: VtParser,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            vt_parser: VtParser::new(),
        }
    }

    pub fn parse_output(&mut self, bytes: &[u8]) -> Vec<Action> {
        self.vt_parser.parse(bytes)
    }

    pub fn run_command(
        &mut self,
        command: String,
        args: Vec<String>,
    ) -> Result<(), Box<dyn Error>> {
        let pty_system = native_pty_system();
        let (cols, rows) = terminal_size().unwrap_or((80, 24));
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);
        cmd.cwd(std::env::current_dir()?);

        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let raw_mode_guard = RawModeGuard::new()?;

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let reader_handle = thread::spawn(move || {
            let mut reader = reader;
            let mut stdout = io::stdout();
            let _ = io::copy(&mut reader, &mut stdout);
            let _ = stdout.flush();
        });

        let _writer_handle = thread::spawn(move || {
            let mut stdin = io::stdin();
            let mut writer = writer;
            let mut buffer = [0u8; 1024];

            loop {
                let read_bytes = match stdin.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => count,
                    Err(_) => break,
                };

                let mut filtered = Vec::with_capacity(read_bytes);
                for &byte in &buffer[..read_bytes] {
                    if is_wrapper_hotkey(byte) {
                        continue;
                    }
                    filtered.push(byte);
                }

                if filtered.is_empty() {
                    continue;
                }

                if writer.write_all(&filtered).is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
        });

        let status = child.wait()?;
        drop(raw_mode_guard);
        let _ = reader_handle.join();

        if status.success() {
            return Ok(());
        }

        std::process::exit(status.exit_code() as i32);
    }
}

pub fn parse_command() -> (String, Vec<String>) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    parse_command_from(args)
}

pub fn parse_command_from(args: Vec<String>) -> (String, Vec<String>) {
    let command = "claude".to_string();
    (command, args)
}

fn is_wrapper_hotkey(byte: u8) -> bool {
    byte == 0x02 || byte == 0x13 || byte == 0x11
}

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> Result<Self, Box<dyn Error>> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::parse_command_from;

    #[test]
    fn parse_command_defaults_to_claude() {
        let (command, args) = parse_command_from(Vec::new());
        assert_eq!(command, "claude");
        assert!(args.is_empty());
    }

    #[test]
    fn parse_command_with_args() {
        let args = vec!["--debug".to_string(), "--model".to_string()];
        let (command, remaining) = parse_command_from(args);
        assert_eq!(command, "claude");
        assert_eq!(
            remaining,
            vec!["--debug".to_string(), "--model".to_string()]
        );
    }
}
