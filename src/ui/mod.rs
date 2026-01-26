pub mod app;
pub mod events;
pub mod footer;
pub mod header;
pub mod popup;
pub mod terminal;
pub mod theme;

use crate::pty::{parse_command, PtySession};
use crate::ui::app::App;
use crate::ui::events::{AppEvent, EventHandler};
use crate::ui::footer::Footer;
use crate::ui::header::Header;
use crate::ui::terminal::TerminalBody;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders, Clear};
use ratatui::{Frame, Terminal};
use std::io;
use std::io::Stdout;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use termwiz::surface::CursorVisibility;

pub fn run() -> io::Result<()> {
    let (mut terminal, guard) = setup_terminal()?;
    let tick_rate = Duration::from_millis(250);
    let mut app = App::new(tick_rate);
    let events = EventHandler::new(tick_rate);
    let (command, args) = parse_command();
    let mut pty_session = PtySession::spawn(command, args, events.sender())
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    app.attach_pty(pty_session.handle());
    if let Ok((cols, rows)) = crossterm::terminal::size() {
        let body = body_rect(Rect {
            x: 0,
            y: 0,
            width: cols,
            height: rows,
        });
        app.on_resize(body.width.max(1), body.height.max(1));
    }

    loop {
        terminal.draw(|frame| draw(frame, &app))?;
        if app.should_quit() {
            break;
        }

        match events.next(tick_rate) {
            Ok(AppEvent::Input(key)) => handle_key(&mut app, key),
            Ok(AppEvent::Tick) => app.on_tick(),
            Ok(AppEvent::Resize(cols, rows)) => {
                let body = body_rect(Rect {
                    x: 0,
                    y: 0,
                    width: cols,
                    height: rows,
                });
                app.on_resize(body.width.max(1), body.height.max(1));
            }
            Ok(AppEvent::PtyOutput) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = pty_session.shutdown();
    drop(guard);
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if matches!(key.code, KeyCode::Esc)
        || (matches!(key.code, KeyCode::Char('q')) && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        app.request_quit();
    } else {
        app.on_key(key);
    }
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.size();
    let (header, body, footer) = layout_regions(area);

    let header_widget = Header::new();
    frame.render_widget(header_widget.widget(), header);
    frame.render_widget(Clear, body);
    if let Some(screen) = app.screen() {
        frame.render_widget(TerminalBody::new(Arc::clone(&screen)), body);
        if body.width > 0 && body.height > 0 {
            if let Ok(screen) = screen.lock() {
                if screen.cursor_visibility() == CursorVisibility::Visible {
                    let (x, y) = screen.cursor_position();
                    let x = body.x + x.min(body.width.saturating_sub(1) as usize) as u16;
                    let y = body.y + y.min(body.height.saturating_sub(1) as usize) as u16;
                    frame.set_cursor(x, y);
                }
            }
        }
    } else {
        frame.render_widget(Block::default().title("Body").borders(Borders::ALL), body);
    }
    let footer_widget = Footer::new();
    frame.render_widget(footer_widget.widget(), footer);

    if app.show_popup() {
        let popup = Block::default().title("Popup").borders(Borders::ALL);
        frame.render_widget(popup, centered_rect(60, 30, frame.size()));
    }
}

fn layout_regions(area: Rect) -> (Rect, Rect, Rect) {
    let header_height = area.height.min(3);
    let footer_height = 3.min(area.height.saturating_sub(header_height));
    let header = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: header_height,
    };
    let footer = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(footer_height),
        width: area.width,
        height: footer_height,
    };
    let body = Rect {
        x: area.x,
        y: area.y + header_height,
        width: area.width,
        height: area.height.saturating_sub(header_height + footer_height),
    };
    (header, body, footer)
}

fn body_rect(area: Rect) -> Rect {
    layout_regions(area).1
}

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    area: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
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
