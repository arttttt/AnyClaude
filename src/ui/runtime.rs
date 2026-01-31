use crate::config::{Config, ConfigStore, ConfigWatcher};
use crate::ipc::IpcLayer;
use crate::proxy::ProxyServer;
use crate::pty::{parse_command, PtySession};
use crate::ui::app::{App, UiCommand};
use crate::ui::events::{AppEvent, EventHandler};
use crate::ui::input::handle_key;
use crate::ui::layout::body_rect;
use crate::ui::render::draw;
use crate::ui::terminal_guard::setup_terminal;
use ratatui::layout::Rect;
use std::io;
use std::time::Duration;
use tokio::runtime::Builder;
use tokio::sync::mpsc;

/// Default debounce delay for config file watching (milliseconds).
const CONFIG_DEBOUNCE_MS: u64 = 200;
const UI_COMMAND_BUFFER: usize = 32;
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const METRICS_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const BACKENDS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub fn run() -> io::Result<()> {
    let (mut terminal, guard) = setup_terminal()?;
    let tick_rate = Duration::from_millis(250);

    // Load initial config
    let config = Config::load().unwrap_or_default();
    let config_path = Config::config_path();
    let config_store = ConfigStore::new(config, config_path);
    let proxy_base_url = config_store.get().proxy.base_url.clone();

    let events = EventHandler::new(tick_rate);
    let async_runtime = Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;

    // Start config file watcher (if it fails, we continue without hot-reload)
    let _config_watcher =
        ConfigWatcher::start(config_store.clone(), events.sender(), CONFIG_DEBOUNCE_MS)
            .map_err(|e| eprintln!("Config watcher failed to start: {}", e))
            .ok();

    let (ui_command_tx, ui_command_rx) = mpsc::channel(UI_COMMAND_BUFFER);
    let mut app = App::new(tick_rate, config_store.clone());
    app.set_ipc_sender(ui_command_tx.clone());
    let proxy_server = ProxyServer::new(config_store.clone())
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    let proxy_handle = proxy_server.handle();
    let backend_state = proxy_server.backend_state();
    let observability = proxy_server.observability();
    let shutdown = proxy_server.shutdown_handle();
    let started_at = std::time::Instant::now();

    let (ipc_client, ipc_server) = IpcLayer::new();
    async_runtime.spawn(async move {
        if let Err(err) = proxy_server.run().await {
            tracing::error!(error = %err, "Proxy server exited");
        }
    });
    async_runtime.spawn(ipc_server.run(
        backend_state.clone(),
        observability,
        shutdown,
        started_at,
    ));

    let bridge_config = config_store.clone();
    let bridge_backend_state = backend_state.clone();
    let bridge_events = events.sender();
    async_runtime.spawn(run_ui_bridge(
        ui_command_rx,
        ipc_client,
        bridge_config,
        bridge_backend_state,
        bridge_events,
    ));

    app.request_status_refresh();
    app.request_backends_refresh();

    let (command, args) = parse_command();
    // Only set BASE_URL to redirect traffic through proxy
    // Claude Code uses its own OAuth for Authorization header
    let env = vec![
        ("ANTHROPIC_BASE_URL".to_string(), proxy_base_url),
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
            Ok(AppEvent::Tick) => {
                app.on_tick();
                if app.should_refresh_status(STATUS_REFRESH_INTERVAL) {
                    app.request_status_refresh();
                }
                if app.popup_kind() == Some(crate::ui::app::PopupKind::Status)
                    && app.should_refresh_metrics(METRICS_REFRESH_INTERVAL)
                {
                    app.request_metrics_refresh(None);
                }
                if app.popup_kind() == Some(crate::ui::app::PopupKind::BackendSwitch)
                    && app.should_refresh_backends(BACKENDS_REFRESH_INTERVAL)
                {
                    app.request_backends_refresh();
                }
            }
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
            Ok(AppEvent::ConfigReload) => {
                app.on_config_reload();
                app.request_config_reload();
                app.request_backends_refresh();
                app.request_status_refresh();
            }
            Ok(AppEvent::IpcStatus(status)) => app.update_status(status),
            Ok(AppEvent::IpcMetrics(metrics)) => app.update_metrics(metrics),
            Ok(AppEvent::IpcBackends(backends)) => app.update_backends(backends),
            Ok(AppEvent::IpcError(message)) => app.set_ipc_error(message),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    proxy_handle.shutdown();
    let _ = pty_session.shutdown();
    drop(guard);
    async_runtime.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

async fn run_ui_bridge(
    mut rx: mpsc::Receiver<UiCommand>,
    ipc_client: crate::ipc::IpcClient,
    config_store: ConfigStore,
    backend_state: crate::backend::BackendState,
    event_tx: std::sync::mpsc::Sender<AppEvent>,
) {
    while let Some(command) = rx.recv().await {
        match command {
            UiCommand::SwitchBackend { backend_id } => {
                match ipc_client.switch_backend(backend_id).await {
                    Ok(Ok(_)) => {
                        if let Ok(status) = ipc_client.get_status().await {
                            let _ = event_tx.send(AppEvent::IpcStatus(status));
                        }
                        if let Ok(backends) = ipc_client.list_backends().await {
                            let _ = event_tx.send(AppEvent::IpcBackends(backends));
                        }
                    }
                    Ok(Err(err)) => {
                        let _ = event_tx.send(AppEvent::IpcError(err.to_string()));
                    }
                    Err(err) => {
                        let _ = event_tx.send(AppEvent::IpcError(err.to_string()));
                    }
                }
            }
            UiCommand::RefreshStatus => match ipc_client.get_status().await {
                Ok(status) => {
                    let _ = event_tx.send(AppEvent::IpcStatus(status));
                }
                Err(err) => {
                    let _ = event_tx.send(AppEvent::IpcError(err.to_string()));
                }
            },
            UiCommand::RefreshMetrics { backend_id } => match ipc_client.get_metrics(backend_id).await
            {
                Ok(metrics) => {
                    let _ = event_tx.send(AppEvent::IpcMetrics(metrics));
                }
                Err(err) => {
                    let _ = event_tx.send(AppEvent::IpcError(err.to_string()));
                }
            },
            UiCommand::RefreshBackends => match ipc_client.list_backends().await {
                Ok(backends) => {
                    let _ = event_tx.send(AppEvent::IpcBackends(backends));
                }
                Err(err) => {
                    let _ = event_tx.send(AppEvent::IpcError(err.to_string()));
                }
            },
            UiCommand::ReloadConfig => match backend_state.update_config(config_store.get()) {
                Ok(()) => {
                    if let Ok(status) = ipc_client.get_status().await {
                        let _ = event_tx.send(AppEvent::IpcStatus(status));
                    }
                    if let Ok(backends) = ipc_client.list_backends().await {
                        let _ = event_tx.send(AppEvent::IpcBackends(backends));
                    }
                }
                Err(err) => {
                    let _ = event_tx.send(AppEvent::IpcError(err.to_string()));
                }
            },
        }
    }
}
