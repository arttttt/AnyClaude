use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use parking_lot::Mutex;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::sync::Arc;

pub struct TerminalGuard {
    cleanup: Arc<Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>>,
}

impl TerminalGuard {
    fn new() -> Self {
        Self {
            cleanup: Arc::new(Mutex::new(None)),
        }
    }

    fn set_cleanup<F: FnOnce() + Send + 'static>(&self, cleanup: F) {
        *self.cleanup.lock() = Some(Box::new(cleanup));
    }

    fn install_panic_hook(&self) {
        let cleanup = Arc::clone(&self.cleanup);
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(cleanup_fn) = cleanup.lock().take() {
                cleanup_fn();
            }
            default_hook(info);
        }));
    }

    fn restore(&self) {
        if let Some(cleanup_fn) = self.cleanup.lock().take() {
            cleanup_fn();
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

pub fn setup_terminal() -> io::Result<(Terminal<CrosstermBackend<Stdout>>, TerminalGuard)> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(EnableBracketedPaste)?;
    stdout.execute(EnableMouseCapture)?;
    stdout.execute(Hide)?;

    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    let guard = TerminalGuard::new();
    guard.set_cleanup(|| {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = stdout.execute(DisableMouseCapture);
        let _ = stdout.execute(DisableBracketedPaste);
        let _ = stdout.execute(LeaveAlternateScreen);
        let _ = stdout.execute(Show);
    });
    guard.install_panic_hook();

    Ok((terminal, guard))
}
