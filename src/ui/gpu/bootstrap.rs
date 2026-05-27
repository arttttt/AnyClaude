//! `run()` — entry point that wires the whole stack together.
//!
//! Owns the config / settings / debug logger / tokio runtime / proxy
//! server / teammate shim setup, prepares spawn params for the Claude
//! Code child, builds the winit event loop with a [`UserEvent`] proxy,
//! and hands off to [`GpuApp`]. The proxy + runtime stay alive for the
//! duration of `event_loop.run_app` and drop cleanly once the user
//! quits.

use std::sync::Arc;

use uuid::Uuid;
use winit::event_loop::EventLoop;

use crate::args::{build_spawn_params, ArgAssembler};
use crate::config::{ClaudeSettingsManager, Config, ConfigStore, DebugLogLevel};
use crate::metrics::{init_global_logger, DebugLogger};
use crate::proxy::ProxyServer;
use crate::shim::TeammateShim;

use super::app::{GpuApp, UserEvent};

/// Entry point for the GPU UI. Routed from `main.rs`.
pub fn run(
    backend_override: Option<String>,
    claude_args: Vec<String>,
) -> std::io::Result<()> {
    // --- Config + backend override ----------------------------------
    let mut config = Config::load()
        .map_err(|e| std::io::Error::other(format!("Failed to load config: {e}")))?;
    if let Some(name) = backend_override {
        config.defaults.active = name;
    }
    let config_path = Config::config_path();
    let config_store = ConfigStore::new(config, config_path);
    let base_proxy_url = config_store.get().proxy.base_url.clone();
    let scrollback_lines = config_store.get().terminal.scrollback_lines;

    // --- Settings manager (seed from config) ------------------------
    let mut settings_manager = ClaudeSettingsManager::new();
    settings_manager.load_from_toml(&config_store.get().claude_settings);

    // --- Session token + tokio runtime ------------------------------
    let session_token = Uuid::new_v4().to_string();
    let async_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    // --- Initial spawn params (URL gets patched after proxy.bind) ---
    let mut spawn = build_spawn_params(
        &claude_args,
        &base_proxy_url,
        &session_token,
        &settings_manager,
        None, // shim PATH injected below once it exists
        None, // proxy port unknown — patched below
    );
    let session_id = spawn.session_id.clone();

    // --- Per-session debug logger -----------------------------------
    let debug_config = {
        let mut cfg = config_store.get().debug_logging.clone();
        if !session_id.is_empty() {
            cfg.file_path = match cfg.file_path.rfind('.') {
                Some(dot) => format!(
                    "{}.{}.{}",
                    &cfg.file_path[..dot],
                    session_id,
                    &cfg.file_path[dot + 1..]
                ),
                None => format!("{}.{session_id}", cfg.file_path),
            };
        }
        cfg
    };
    let debug_logger = Arc::new(DebugLogger::new(debug_config));
    init_global_logger(debug_logger.clone());

    // --- Proxy server + bind ----------------------------------------
    let mut proxy_server = ProxyServer::new(
        config_store.clone(),
        debug_logger.clone(),
        Some(session_token.clone()),
    )
    .map_err(|e| std::io::Error::other(e.to_string()))?;
    let (actual_addr, actual_base_url) = async_runtime
        .block_on(async { proxy_server.try_bind(&config_store).await })
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    // Patch ANTHROPIC_BASE_URL with the actually-bound port.
    for (key, value) in &mut spawn.env {
        if key == "ANTHROPIC_BASE_URL" {
            *value = actual_base_url.clone();
        }
    }

    // --- Teammate shim (optional — config-driven) -------------------
    let log_enabled = config_store.get().debug_logging.level != DebugLogLevel::Off;
    let teammate_shim =
        match TeammateShim::create(actual_addr.port(), &session_token, &session_id, log_enabled) {
            Ok(shim) => {
                crate::metrics::app_log(
                    "gpu_runtime",
                    &format!(
                        "Agent team routing enabled, shim dir prepended to PATH. tmux log: {}",
                        shim.tmux_log_path().display()
                    ),
                );
                Some(shim)
            }
            Err(e) => {
                crate::metrics::app_log(
                    "gpu_runtime",
                    &format!("Agent team routing disabled: {e}"),
                );
                None
            }
        };

    // --- Inject subagent hooks into spawn.args ----------------------
    spawn
        .args
        .extend(ArgAssembler::new().with_subagent_hooks(actual_addr.port()).build());

    // --- Inject shim PATH into spawn.env ----------------------------
    if let Some(ref shim) = teammate_shim {
        let (key, value) = shim.path_env();
        if let Some(existing) = spawn.env.iter_mut().find(|(k, _)| k == &key) {
            existing.1 = value;
        } else {
            spawn.env.push((key, value));
        }
    }

    // --- Capture proxy state and run proxy as a tokio task ----------
    let backend_state = proxy_server.backend_state();
    let subagent_backend = proxy_server.subagent_backend();
    let teammate_backend = proxy_server.teammate_backend();
    let observability = proxy_server.observability();
    let _proxy_task = async_runtime.spawn(async move {
        if let Err(e) = proxy_server.run().await {
            crate::metrics::app_log_error("gpu_runtime", "Proxy server exited", &e.to_string());
        }
    });

    // --- Hand off to the winit event loop ---------------------------
    let _ = scrollback_lines; // Reserved for future grid configuration.
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let proxy = event_loop.create_proxy();
    let mut app = GpuApp::new(
        proxy,
        spawn.command,
        spawn.args,
        spawn.env,
        backend_state,
        subagent_backend,
        teammate_backend,
        observability,
        settings_manager,
    );
    event_loop
        .run_app(&mut app)
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    // Tokio runtime + teammate shim drop here, shutting the proxy
    // task down and cleaning up the shim's temp directory.
    drop(teammate_shim);
    drop(async_runtime);
    Ok(())
}
