#[cfg(unix)]
mod pty_passthrough {
    use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
    use std::error::Error;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    fn run_command_and_capture(command: &str, args: &[&str]) -> Result<Vec<u8>, Box<dyn Error>> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");

        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let master = pair.master;
        let mut reader = master.try_clone_reader()?;
        let writer = master.take_writer()?;
        drop(writer);

        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;

        let status = child.wait()?;
        assert!(status.success());

        Ok(output)
    }

    struct InteractivePty {
        master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
        writer: Option<Box<dyn Write + Send>>,
        output: Arc<Mutex<Vec<u8>>>,
        reader_handle: thread::JoinHandle<()>,
        child: Box<dyn portable_pty::Child + Send>,
    }

    impl InteractivePty {
        fn spawn_shell(cols: u16, rows: u16) -> Result<Self, Box<dyn Error>> {
            let pty_system = native_pty_system();
            let pair = pty_system.openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;

            let mut cmd = CommandBuilder::new("sh");
            cmd.env("TERM", "xterm-256color");
            cmd.env("PS1", "");

            let child = pair.slave.spawn_command(cmd)?;
            drop(pair.slave);

            let master = pair.master;
            let reader = master.try_clone_reader()?;
            let writer = master.take_writer()?;
            let master = Arc::new(Mutex::new(master));

            let output = Arc::new(Mutex::new(Vec::new()));
            let output_clone = Arc::clone(&output);
            let reader_handle = thread::spawn(move || {
                let mut reader = reader;
                let mut buffer = [0u8; 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            if let Ok(mut output) = output_clone.lock() {
                                output.extend_from_slice(&buffer[..count]);
                            }
                        }
                        Err(_) => break,
                    }
                }
            });

            Ok(Self {
                master,
                writer: Some(writer),
                output,
                reader_handle,
                child,
            })
        }

        fn write_line(&mut self, line: &str) -> Result<(), Box<dyn Error>> {
            if let Some(writer) = &mut self.writer {
                writer.write_all(line.as_bytes())?;
                writer.flush()?;
            }
            Ok(())
        }

        fn resize(&self, cols: u16, rows: u16) -> Result<(), Box<dyn Error>> {
            if let Ok(master) = self.master.lock() {
                master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })?;
            }
            Ok(())
        }

        fn wait_for_output(&self, needle: &str, timeout: Duration) -> bool {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                if let Ok(output) = self.output.lock() {
                    let text = String::from_utf8_lossy(&output);
                    if text.contains(needle) {
                        return true;
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
            false
        }

        fn shutdown(mut self) -> Result<(), Box<dyn Error>> {
            self.writer.take();
            let status = self.child.wait()?;
            drop(self.master);
            let _ = self.reader_handle.join();
            assert!(status.success());
            Ok(())
        }
    }

    #[test]
    fn spawn_command_captures_output() -> Result<(), Box<dyn Error>> {
        let output = run_command_and_capture("sh", &["-c", "printf 'ready'"])?;
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("ready"));
        Ok(())
    }

    #[test]
    fn input_echoes_back() -> Result<(), Box<dyn Error>> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new("cat");
        cmd.env("TERM", "xterm-256color");

        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let master = pair.master;
        let mut reader = master.try_clone_reader()?;
        let mut writer = master.take_writer()?;

        writer.write_all(b"ping\n")?;
        writer.flush()?;
        drop(writer);

        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        let status = child.wait()?;
        assert!(status.success());

        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("ping"));
        Ok(())
    }

    #[test]
    fn resize_updates_shell_size() -> Result<(), Box<dyn Error>> {
        let mut session = InteractivePty::spawn_shell(80, 24)?;
        session.write_line("stty size\n")?;
        assert!(session.wait_for_output("24 80", Duration::from_secs(2)));

        session.resize(100, 40)?;
        session.write_line("stty size\n")?;
        assert!(session.wait_for_output("40 100", Duration::from_secs(2)));

        session.write_line("exit\n")?;
        session.shutdown()?;
        Ok(())
    }

    /// Verify that ESC+DEL (Option+Backspace) passes through PTY correctly
    /// even when the child process sets raw mode.
    #[test]
    fn esc_del_passes_through_pty() -> Result<(), Box<dyn Error>> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Use Python to: set raw mode, read 2 bytes, print their hex, exit.
        let mut cmd = CommandBuilder::new("python3");
        cmd.args([
            "-c",
            concat!(
                "import sys, os, tty, termios; ",
                "old = termios.tcgetattr(0); ",
                "tty.setraw(0); ",
                "os.write(1, b'READY\\n'); ",
                "data = os.read(0, 10); ",
                "termios.tcsetattr(0, termios.TCSADRAIN, old); ",
                "os.write(1, ('HEX:' + data.hex() + '\\n').encode())",
            ),
        ]);
        cmd.env("TERM", "xterm-256color");

        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let master = pair.master;
        let mut reader = master.try_clone_reader()?;
        let mut writer = master.take_writer()?;

        // Wait for READY signal
        let mut output = Vec::new();
        let mut buf = [0u8; 256];
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() > deadline {
                panic!("Timeout waiting for READY from child");
            }
            let n = reader.read(&mut buf)?;
            output.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&output).contains("READY") {
                break;
            }
        }

        // Send ESC + DEL (Option+Backspace)
        writer.write_all(&[0x1b, 0x7f])?;
        writer.flush()?;

        // Read the hex output
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() > deadline {
                let text = String::from_utf8_lossy(&output);
                panic!("Timeout waiting for HEX output. Got so far: {}", text);
            }
            let n = reader.read(&mut buf)?;
            output.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&output);
            if let Some(pos) = text.find("HEX:") {
                let hex_line = &text[pos..];
                // ESC=1b, DEL=7f
                assert!(
                    hex_line.contains("1b7f"),
                    "Expected ESC+DEL (1b7f) but got: {}",
                    hex_line.trim()
                );
                break;
            }
        }

        drop(writer);
        let _ = child.wait();
        Ok(())
    }

    #[test]
    #[ignore]
    fn benchmark_vt_rendering() {
        let mut emu = anyclaude::pty::emulator::create(24, 80, 0);
        let payload = b"\x1b[2J\x1b[HThe quick brown fox jumps over the lazy dog\n";
        let iterations = 20_000;
        let start = Instant::now();
        for _ in 0..iterations {
            emu.process(payload);
        }
        let elapsed = start.elapsed();
        let per_sec = iterations as f64 / elapsed.as_secs_f64();
        eprintln!("VT render benchmark: {:.0} iterations/sec", per_sec);
    }
}
