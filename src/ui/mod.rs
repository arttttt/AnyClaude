pub mod app;
pub mod events;
pub mod footer;
pub mod header;
pub mod popup;
pub mod theme;

use crate::ui::app::App;
use crate::ui::events::{AppEvent, EventHandler};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{KeyCode, KeyEvent};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders};
use ratatui::{Frame, Terminal};
use std::io;
use std::io::Stdout;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub fn run() -> io::Result<()> {
    let (mut terminal, guard) = setup_terminal()?;
    let tick_rate = Duration::from_millis(250);
    let mut app = App::new(tick_rate);
    let events = EventHandler::new(tick_rate);

    loop {
        terminal.draw(|frame| draw(frame, &app))?;
        if app.should_quit() {
            break;
        }

        match events.next(tick_rate) {
            Ok(AppEvent::Input(key)) => handle_key(&mut app, key),
            Ok(AppEvent::Tick) => app.on_tick(),
            Ok(AppEvent::Resize(cols, rows)) => app.on_resize(cols, rows),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    drop(guard);
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
        app.request_quit();
    } else {
        app.on_key(key);
    }
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(frame.size());

    frame.render_widget(Block::default().title("Header").borders(Borders::ALL), chunks[0]);
    frame.render_widget(Block::default().title("Body").borders(Borders::ALL), chunks[1]);
    frame.render_widget(Block::default().title("Footer").borders(Borders::ALL), chunks[2]);

    if app.show_popup() {
        let popup = Block::default().title("Popup").borders(Borders::ALL);
        frame.render_widget(popup, centered_rect(60, 30, frame.size()));
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

struct TerminalGuard {
    cleanup: Arc<Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>>,
}

impl TerminalGuard {
    fn new() -> Self {
        Self {
            cleanup: Arc::new(Mutex::new(None)),
        }
    }

    fn set_cleanup<F: FnOnce() + Send + 'static>(&self, cleanup: F) {
        if let Ok(mut slot) = self.cleanup.lock() {
            *slot = Some(Box::new(cleanup));
        }
    }

    fn install_panic_hook(&self) {
        let cleanup = Arc::clone(&self.cleanup);
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Ok(mut slot) = cleanup.lock() {
                if let Some(cleanup) = slot.take() {
                    cleanup();
                }
            }
            default_hook(info);
        }));
    }

    fn restore(&self) {
        if let Ok(mut slot) = self.cleanup.lock() {
            if let Some(cleanup) = slot.take() {
                cleanup();
            }
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

fn setup_terminal() -> io::Result<(Terminal<CrosstermBackend<Stdout>>, TerminalGuard)> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(Hide)?;

    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    let guard = TerminalGuard::new();
    guard.set_cleanup(|| {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = stdout.execute(LeaveAlternateScreen);
        let _ = stdout.execute(Show);
    });
    guard.install_panic_hook();

    Ok((terminal, guard))
}
