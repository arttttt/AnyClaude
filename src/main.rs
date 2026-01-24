use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size as terminal_size};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::error::Error;
use std::io::{self, Write};
use std::thread;

fn main() -> Result<(), Box<dyn Error>> {
    let (command, args) = parse_command();
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

    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    enable_raw_mode()?;

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
        let _ = io::copy(&mut stdin, &mut writer);
        let _ = writer.flush();
    });

    let status = child.wait()?;
    let _ = disable_raw_mode();
    let _ = reader_handle.join();

    if status.success() {
        return Ok(());
    }

    std::process::exit(status.exit_code() as i32);
}

fn parse_command() -> (String, Vec<String>) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    parse_command_from(args)
}

fn parse_command_from(args: Vec<String>) -> (String, Vec<String>) {
    let command = "claude".to_string();
    (command, args)
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
