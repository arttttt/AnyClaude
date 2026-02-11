use crate::clipboard::{ClipboardContent, ClipboardHandler};
use crate::config::{save_claude_settings, Config, ConfigStore};
use crate::error::{ErrorCategory, ErrorSeverity};
use crate::ipc::IpcLayer;
use crate::metrics::{init_global_logger, DebugLogger};
use crate::proxy::ProxyServer;
use crate::pty::{PtySession, PtySpawnConfig, SessionMode, SpawnParams};
use crate::shutdown::{ShutdownCoordinator, ShutdownPhase};
use crate::ui::app::{App, UiCommand};
use crate::ui::events::{AppEvent, EventHandler};
use crate::ui::history::HistoryEntry;
use crate::ui::input::{classify_key, InputAction};
use crate::ui::layout::body_rect;
use crate::ui::render::draw;
use crate::ui::selection::GridPos;
use crate::ui::terminal_guard::setup_terminal;
use term_input::MouseEvent;
use ratatui::layout::Rect;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Builder;
use tokio::sync::mpsc;

const UI_COMMAND_BUFFER: usize = 32;
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const METRICS_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const BACKENDS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub fn run(backend_override: Option<String>, claude_args: Vec<String>) -> io::Result<()> {
    let (mut terminal, guard) = setup_terminal()?;
    let tick_rate = Duration::from_millis(250);

    // Load initial config and apply backend override
    let mut config = Config::load().map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData, format!("Failed to load config: {}", e))
    })?;
    if let Some(backend_name) = backend_override {
        config.defaults.active = backend_name;
    }
    let config_path = Config::config_path();
    let config_store = ConfigStore::new(config, config_path);

    // Create shutdown coordinator for graceful shutdown
    let shutdown_coordinator = ShutdownCoordinator::new();
    let shutdown_handle = shutdown_coordinator.handle();

    let events = EventHandler::new(tick_rate, shutdown_handle.clone());
    let async_runtime = Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| io::Error::other(err.to_string()))?;

    // Config file watching removed to avoid race conditions with CLI overrides.
    // Config is loaded once at startup and remains static for the session.

    let debug_logger = Arc::new(DebugLogger::new(config_store.get().debug_logging.clone()));
    init_global_logger(debug_logger.clone());

    let (ui_command_tx, ui_command_rx) = mpsc::channel(UI_COMMAND_BUFFER);
    let mut app = App::new(config_store.clone());
    app.set_ipc_sender(ui_command_tx.clone());

    let mut proxy_server = ProxyServer::new(config_store.clone(), debug_logger.clone())
        .map_err(|err| io::Error::other(err.to_string()))?;
    
    // Try to bind and get the actual port, updating the base URL
    let (_actual_addr, actual_base_url) = async_runtime.block_on(async {
        proxy_server.try_bind(&config_store).await
    }).map_err(|err| io::Error::other(err.to_string()))?;
    
    let proxy_handle = proxy_server.handle();
    let backend_state = proxy_server.backend_state();

    // Wire history provider: converts SwitchLogEntry → HistoryEntry at the boundary
    {
        let bs = backend_state.clone();
        let provider = Arc::new(move || {
            bs.get_switch_log()
                .into_iter()
                .map(|e| HistoryEntry {
                    timestamp: e.timestamp,
                    from_backend: e.old_backend,
                    to_backend: e.new_backend,
                })
                .collect()
        });
        app.set_history_provider(provider);
    }

    let observability = proxy_server.observability();
    let shutdown = proxy_server.shutdown_handle();
    let transformer_registry = proxy_server.transformer_registry();
    let started_at = std::time::Instant::now();

    let (ipc_client, ipc_server) = IpcLayer::create();
    async_runtime.spawn(async move {
        if let Err(err) = proxy_server.run().await {
            crate::metrics::app_log_error("runtime", "Proxy server exited", &err.to_string());
        }
    });
    async_runtime.spawn(ipc_server.run(
        backend_state.clone(),
        observability,
        debug_logger,
        shutdown,
        started_at,
        transformer_registry,
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

    // Spawn OS signal handler
    let signal_events = events.sender();
    async_runtime.spawn(async move {
        wait_for_os_signal().await;
        let _ = signal_events.send(AppEvent::Shutdown);
    });

    app.request_status_refresh();
    app.request_backends_refresh();

    let scrollback_lines = config_store.get().terminal.scrollback_lines;
    let spawn_config = PtySpawnConfig::new(
        "claude".to_string(),
        claude_args,
        actual_base_url,
    );

    for warning in spawn_config.warnings() {
        app.error_registry().record(
            ErrorSeverity::Warning,
            ErrorCategory::Process,
            warning,
        );
    }

    let initial_env = app.settings_manager().to_env_vars();
    let initial_args = app.settings_manager().to_cli_args();
    let initial = spawn_config.build(initial_env, initial_args, SessionMode::Initial);
    let mut pty_session = PtySession::spawn(
        spawn_config.command().to_string(),
        initial.args,
        initial.env,
        scrollback_lines,
        events.sender(),
        app.pty_generation(),
    )
    .map_err(|err| io::Error::other(err.to_string()))?;

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

    // Initialize clipboard handler (may fail on headless systems)
    let mut clipboard = ClipboardHandler::new().ok();

    // When true, a failed --resume restart can be retried with --session-id.
    // Set on PtyRestart, cleared after retry or on successful attach.
    let mut restart_can_retry = false;
    // Deferred mouse-down anchor: start_selection only on first Drag,
    // not on Down, to avoid selecting a single character on plain click.
    let mut mouse_down_pos: Option<GridPos> = None;

    loop {
        terminal.draw(|frame| draw(frame, &app))?;
        if app.should_quit() {
            break;
        }

        match events.next(tick_rate) {
            Ok(AppEvent::Key(key)) => {
                // Reset scrollback and clear selection on any key input
                app.reset_scrollback();
                app.clear_selection();
                match classify_key(&mut app, &key) {
                    InputAction::Forward => {
                        app.send_input(&key.raw);
                    }
                    InputAction::ImagePaste => {
                        handle_image_paste(&mut app, &mut clipboard);
                    }
                    InputAction::None => {}
                }
            }
            Ok(AppEvent::Mouse(mouse)) => {
                let (col, row) = mouse.position();
                // 1. Scroll — always handled locally
                if mouse.is_scroll() {
                    app.clear_selection();
                    match mouse {
                        MouseEvent::ScrollUp { .. } => app.scroll_up(3),
                        MouseEvent::ScrollDown { .. } => app.scroll_down(3),
                        _ => {}
                    }
                }
                // 2. PTY mouse tracking — forward to child process
                else if app.mouse_tracking() {
                    app.send_input(&mouse.to_x10_bytes());
                }
                // 3. Wrapper text selection — click+drag
                else {
                    match mouse {
                        MouseEvent::Down { button: term_input::MouseButton::Left, .. } => {
                            app.clear_selection();
                            mouse_down_pos = screen_to_grid(col, row);
                        }
                        MouseEvent::Drag { button: term_input::MouseButton::Left, .. } => {
                            if let Some(pos) = screen_to_grid(col, row) {
                                if let Some(anchor) = mouse_down_pos {
                                    if anchor != pos {
                                        // Cursor moved to a different cell — start selection
                                        mouse_down_pos = None;
                                        app.start_selection(anchor);
                                        app.update_selection(pos);
                                    }
                                    // Same cell — wait for real movement
                                } else {
                                    // Selection already started — update end position
                                    app.update_selection(pos);
                                }
                            }
                        }
                        MouseEvent::Up { .. } => {
                            mouse_down_pos = None;
                            if let Some(text) = app.finish_selection() {
                                if !text.is_empty() {
                                    if let Some(clip) = &mut clipboard {
                                        if let Err(err) = clip.set_text(&text) {
                                            app.error_registry().record(
                                                ErrorSeverity::Warning,
                                                ErrorCategory::Process,
                                                &err,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(AppEvent::Paste(text)) => {
                if text.trim().is_empty() {
                    // Empty paste likely means image in clipboard - check for image
                    handle_image_paste(&mut app, &mut clipboard);
                } else {
                    app.on_paste(&text);
                }
            }
            Ok(AppEvent::ImagePaste(path)) => app.on_image_paste(&path),
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
            Ok(AppEvent::PtyOutput) => {
                if app.on_pty_output() {
                    // PTY just reached Ready — clear retry flag.
                    restart_can_retry = false;
                }
            }
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
            Ok(AppEvent::ConfigError(message)) => {
                app.error_registry().record_with_details(
                    ErrorSeverity::Warning,
                    ErrorCategory::Config,
                    "Config reload failed",
                    Some(message),
                );
            }
            Ok(AppEvent::PtyError(error)) => {
                app.error_registry().record_with_details(
                    ErrorSeverity::Critical,
                    ErrorCategory::Process,
                    error.user_message(),
                    Some(error.details()),
                );
            }
            Ok(AppEvent::Shutdown) => {
                app.request_quit();
            }
            Ok(AppEvent::ProcessExit { pty_generation }) => {
                // Guaranteed reset: capture and clear retry flag up front.
                let can_retry = restart_can_retry;
                restart_can_retry = false;

                if pty_generation != app.pty_generation() {
                    // Stale ProcessExit from an old PTY instance — ignore.
                } else if app.pty_lifecycle.is_restarting() {
                    // Current generation but lifecycle is restarting — ignore.
                } else if app.has_restarted() && !app.pty_lifecycle.is_ready() {
                    // Process exited before reaching Ready after a restart.
                    if can_retry {
                        // --resume failed (likely no conversation yet).
                        // Retry with --session-id to start fresh session.
                        let env_vars = app.settings_manager().to_env_vars();
                        let cli_args = app.settings_manager().to_cli_args();
                        let params = spawn_config.build(env_vars, cli_args, SessionMode::Initial);
                        respawn_pty(
                            &mut app,
                            &mut pty_session,
                            &spawn_config,
                            params,
                            scrollback_lines,
                            &events,
                        );
                    } else {
                        app.dispatch_pty(crate::ui::pty::PtyIntent::SpawnFailed);
                        app.error_registry().record(
                            ErrorSeverity::Critical,
                            ErrorCategory::Process,
                            "Claude Code exited during restart",
                        );
                    }
                } else {
                    app.error_registry().record(
                        ErrorSeverity::Info,
                        ErrorCategory::Process,
                        "Claude Code process exited",
                    );
                    app.request_quit();
                }
            }
            Ok(AppEvent::PtyRestart { env_vars, cli_args }) => {
                // Lifecycle is already Restarting (set in apply_settings).
                // Always try --resume first. If the session hasn't had any
                // interaction yet, --resume will fail (no conversation to
                // resume). The ProcessExit safety net will then retry with
                // --session-id (SessionMode::Initial).
                restart_can_retry = true;
                let params = spawn_config.build(env_vars, cli_args, SessionMode::Resume);
                respawn_pty(
                    &mut app,
                    &mut pty_session,
                    &spawn_config,
                    params,
                    scrollback_lines,
                    &events,
                );
                if !app.pty_lifecycle.is_attached() {
                    // Spawn failed immediately — no point retrying.
                    restart_can_retry = false;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Signal shutdown to all components
    shutdown_coordinator.signal();

    // Phase 2: Stop input
    shutdown_coordinator.advance(ShutdownPhase::StoppingInput);
    drop(events);

    // Phase 3 & 4: Terminate child and close proxy
    shutdown_coordinator.advance(ShutdownPhase::TerminatingChild);
    proxy_handle.shutdown();
    // pty_session.shutdown() is handled by Drop when pty_session goes out of scope.
    drop(pty_session);

    // Phase 5: Cleanup
    shutdown_coordinator.advance(ShutdownPhase::Cleanup);
    drop(guard);
    async_runtime.shutdown_timeout(Duration::from_secs(2));

    shutdown_coordinator.advance(ShutdownPhase::Complete);
    crate::metrics::app_log("runtime", "Shutdown complete");
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
            UiCommand::RestartPty {
                env_vars,
                cli_args,
                settings_toml,
            } => {
                // Persist settings to config file before restarting.
                // Only restart if save succeeds — otherwise user would lose settings on next launch.
                let config_path = config_store.path().to_path_buf();
                match save_claude_settings(&config_path, &settings_toml) {
                    Ok(()) => {
                        let _ = event_tx.send(AppEvent::PtyRestart { env_vars, cli_args });
                    }
                    Err(err) => {
                        let _ = event_tx.send(AppEvent::IpcError(format!(
                            "Failed to save settings: {}",
                            err
                        )));
                    }
                }
            }
        }
    }
}

/// Shut down the current PTY and spawn a new one with the given spawn params.
///
/// On success, attaches the new PTY and resizes it to the current terminal.
/// On failure, dispatches `SpawnFailed` and records an error.
fn respawn_pty(
    app: &mut App,
    pty_session: &mut PtySession,
    spawn_config: &PtySpawnConfig,
    params: SpawnParams,
    scrollback_lines: usize,
    events: &EventHandler,
) {
    // Increment generation BEFORE shutdown so that any ProcessExit from the
    // old reader thread (which carries the old generation) will be stale.
    let gen = app.next_pty_generation();
    app.detach_pty();
    let _ = pty_session.shutdown();

    match PtySession::spawn(
        spawn_config.command().to_string(),
        params.args,
        params.env,
        scrollback_lines,
        events.sender(),
        gen,
    ) {
        Ok(new_session) => {
            app.attach_pty(new_session.handle());
            if let Ok((cols, rows)) = crossterm::terminal::size() {
                let body = body_rect(Rect {
                    x: 0,
                    y: 0,
                    width: cols,
                    height: rows,
                });
                app.on_resize(body.width.max(1), body.height.max(1));
            }
            *pty_session = new_session;
        }
        Err(err) => {
            app.dispatch_pty(crate::ui::pty::PtyIntent::SpawnFailed);
            app.error_registry().record_with_details(
                ErrorSeverity::Critical,
                ErrorCategory::Process,
                "Failed to restart Claude Code",
                Some(err.to_string()),
            );
        }
    }
}

/// Convert screen coordinates to grid coordinates within the terminal body.
/// Returns None if the position is outside the body area.
fn screen_to_grid(col: u16, row: u16) -> Option<GridPos> {
    let (cols, rows) = crossterm::terminal::size().ok()?;
    let body = body_rect(Rect {
        x: 0,
        y: 0,
        width: cols,
        height: rows,
    });
    if col < body.x
        || row < body.y
        || col >= body.x.saturating_add(body.width)
        || row >= body.y.saturating_add(body.height)
    {
        return None;
    }
    Some(GridPos {
        row: row - body.y,
        col: col - body.x,
    })
}

/// Handle image paste request by checking clipboard for image content.
fn handle_image_paste(app: &mut App, clipboard: &mut Option<ClipboardHandler>) {
    let Some(clip) = clipboard else {
        return;
    };

    match clip.get_content() {
        ClipboardContent::Image(path) => {
            app.on_image_paste(&path);
        }
        ClipboardContent::Text(text) => {
            // Fall back to text paste if no image
            app.on_paste(&text);
        }
        ClipboardContent::Empty => {}
    }

    if let Some(err) = clip.take_error() {
        app.error_registry().record(
            ErrorSeverity::Warning,
            ErrorCategory::Process,
            &err,
        );
    }
}

/// Wait for OS shutdown signals (SIGTERM, SIGINT).
async fn wait_for_os_signal() {
    use tokio::signal;

    #[cfg(unix)]
    {
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler");

        tokio::select! {
            _ = signal::ctrl_c() => {
                crate::metrics::app_log("runtime", "Received SIGINT");
            }
            _ = sigterm.recv() => {
                crate::metrics::app_log("runtime", "Received SIGTERM");
            }
        }
    }

    #[cfg(not(unix))]
    {
        signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
        crate::metrics::app_log("runtime", "Received Ctrl+C");
    }
}
