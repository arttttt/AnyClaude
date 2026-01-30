use crate::config::{Config, ConfigStore, ConfigWatcher};
use crate::pty::{parse_command, PtySession};
use crate::ui::app::App;
use crate::ui::events::{AppEvent, EventHandler};
use crate::ui::input::handle_key;
use crate::ui::layout::body_rect;
use crate::ui::render::draw;
use crate::ui::terminal_guard::setup_terminal;
use ratatui::layout::Rect;
use std::io;
use std::time::Duration;
use uuid::Uuid;

/// Default debounce delay for config file watching (milliseconds).
const CONFIG_DEBOUNCE_MS: u64 = 200;

pub fn run() -> io::Result<()> {
    let (mut terminal, guard) = setup_terminal()?;
    let tick_rate = Duration::from_millis(250);

    // Load initial config
    let config = Config::load().unwrap_or_default();
    let config_path = Config::config_path();
    let config_store = ConfigStore::new(config, config_path);
    let proxy_base_url = config_store.get().proxy.base_url.clone();
    let session_token = Uuid::new_v4().to_string();

    let events = EventHandler::new(tick_rate);

    // Start config file watcher (if it fails, we continue without hot-reload)
    let _config_watcher =
        ConfigWatcher::start(config_store.clone(), events.sender(), CONFIG_DEBOUNCE_MS)
            .map_err(|e| eprintln!("Config watcher failed to start: {}", e))
            .ok();

    let mut app = App::new(tick_rate, config_store);
    let (command, args) = parse_command();
    let env = vec![
        ("ANTHROPIC_BASE_URL".to_string(), proxy_base_url),
        ("ANTHROPIC_AUTH_TOKEN".to_string(), session_token.clone()),
    ];
    let mut pty_session = PtySession::spawn(command, args, env, events.sender())
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
            Ok(AppEvent::ConfigReload) => app.on_config_reload(),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = pty_session.shutdown();
    drop(guard);
    Ok(())
}
