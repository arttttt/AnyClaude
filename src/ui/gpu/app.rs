//! Winit `ApplicationHandler` for the GPU UI.
//!
//! Current scope (C2): spawn a shell PTY, feed its bytes into a
//! `term_core::VtEmulator`, render the emulator's snapshot through
//! `term_gpu::populate_panel`. Keyboard / scroll / selection / clipboard
//! land in C3; header / footer chrome in C4-C5; popup overlays in
//! C6-C9. The `--gpu` CLI flag routes here for incremental
//! verification; it is removed in the C10 cutover commit.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::{abortable, AbortHandle};
use futures_timer::Delay;
use glam::Vec2;
use term_clipboard::{
    get_image_filepaths_from_paths, pick_best_image, save_image_to_temp,
    should_insert_text_on_paste, Clipboard, ClipboardContent,
};
use term_core::{create_emulator, AnsiPalette, MouseMode, TerminalEmulator};
use term_gpu::{
    build_cursor_rect, decay_velocity, encode_key, encode_paste, expand_line, expand_word,
    measure_cell_metrics, measure_label_width, populate_panel, push_label, push_selection_rects,
    selection_to_text, shell_quote_path, CellMetrics, CellPoint, FontFamily, FontSystem,
    GlyphAtlas, GlyphInstance, GpuRenderer, PanelRect, RectInstance, RenderLayer, ScrollState,
    ScrollVelocity, Selection, Style, SwashCache, TextShapeCache, Weight, GESTURE_END_TIMEOUT,
    MOMENTUM_FRAME_INTERVAL, MOMENTUM_MIN_VELOCITY, MOMENTUM_THRESHOLD, NUM_PIXELS_PER_LINE,
};
use uuid::Uuid;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{
    ElementState, Modifiers, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use mvi::Store;

use crate::args::build_spawn_params;
use crate::backend::{AgentBackendState, BackendState};
use crate::config::{
    save_claude_settings, Backend, ClaudeSettingsManager, Config, ConfigStore, DebugLogLevel,
    SettingsFieldSnapshot,
};
use crate::metrics::{init_global_logger, DebugLogger, ObservabilityHub};
use crate::proxy::ProxyServer;
use crate::shim::TeammateShim;
use crate::ui::backend_switch::{
    BackendPopupSection, BackendSwitchActor, BackendSwitchIntent, BackendSwitchState,
};
use crate::ui::gpu::pty::ChildPty;
use crate::ui::history::{HistoryActor, HistoryDialogState, HistoryEntry, HistoryIntent};
use crate::ui::settings::{SettingsActor, SettingsDialogState, SettingsIntent};

const INITIAL_W: f32 = 1200.0;
const INITIAL_H: f32 = 800.0;
const FONT_SIZE: f32 = 14.0;
const SCROLLBACK_LINES: usize = 1000;
const INITIAL_GRID_COLS: usize = 80;
const INITIAL_GRID_ROWS: usize = 24;

/// Follow-mode tolerance: scroll offsets within this many logical
/// pixels of the bottom count as "at bottom" — so a tiny stale offset
/// from the last momentum tick doesn't keep follow mode off.
const SCROLL_BOTTOM_EPSILON: f32 = 0.5;

/// Maximum elapsed time between consecutive mouse presses at the same
/// cell for them to count as a double / triple click. macOS's system
/// default is ~500 ms; 400 ms is a comfortable middle ground.
const MULTI_CLICK_THRESHOLD_MS: u128 = 400;

/// Top chrome reserved for the header — backend / Reqs / Uptime /
/// Session etc. live here. Terminal area starts immediately below.
const HEADER_HEIGHT_LOGICAL: f32 = 24.0;

/// Bottom chrome reserved for the footer — hotkey hints + version.
/// Terminal area ends immediately above.
const FOOTER_HEIGHT_LOGICAL: f32 = 22.0;

/// Footer hint text — all the app-level shortcuts the GPU UI knows
/// about. Some land in later Phase 5 commits (Cmd+B switch / Cmd+H
/// history / Cmd+E settings / Cmd+R restart all in C7-C10); Cmd+Q
/// and Cmd+C / Cmd+V already work as of C3.
const FOOTER_HINTS: &str =
    " Cmd+B: Switch │ Cmd+H: History │ Cmd+E: Settings │ Cmd+R: Restart │ Cmd+Q: Quit";

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Font size for header chrome text (variable-width SansSerif), in
/// logical pixels. Smaller than the terminal font so the header
/// stays unobtrusive.
const CHROME_FONT_SIZE: f32 = 12.0;

/// How long the "Session ID copied!" flash stays visible after a
/// successful copy click.
const SESSION_COPY_FLASH: Duration = Duration::from_millis(1500);

/// Dim foreground for chrome labels.
const CHROME_TEXT_COLOR: [f32; 4] = [0.55, 0.55, 0.55, 1.0];
/// Highlight color for the "Session ID copied!" flash. Same green
/// the legacy ratatui chrome uses for STATUS_OK.
const CHROME_FLASH_COLOR: [f32; 4] = [0.4, 0.85, 0.4, 1.0];

/// Popup background color (dark grey with full alpha — opaque so the
/// content underneath is hidden).
const POPUP_BG_COLOR: [f32; 4] = [0.12, 0.12, 0.14, 1.0];
/// Popup item highlight (selected row).
const POPUP_HIGHLIGHT_COLOR: [f32; 4] = [0.22, 0.30, 0.42, 1.0];
/// Popup border / frame (subtle).
const POPUP_BORDER_COLOR: [f32; 4] = [0.30, 0.30, 0.35, 1.0];
/// Popup drop shadow color — soft black at 45% opacity.
const POPUP_SHADOW_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.45];
/// Drop shadow blur radius (logical px).
const POPUP_SHADOW_BLUR: f32 = 24.0;
/// Drop shadow downward offset (logical px).
const POPUP_SHADOW_OFFSET_Y: f32 = 8.0;
/// Rounded-corner radius for popup background.
const POPUP_CORNER_RADIUS: f32 = 6.0;
/// Popup line height (logical px) for items.
const POPUP_LINE_HEIGHT: f32 = 22.0;
/// Popup font size for items.
const POPUP_FONT_SIZE: f32 = 13.0;
/// Padding around popup content (logical px).
const POPUP_PADDING: f32 = 12.0;
/// Default popup width when items are short.
const POPUP_MIN_WIDTH: f32 = 280.0;

/// User event delivered to the winit loop. Drives redraws in response
/// to PTY output and scroll momentum without polling.
#[derive(Debug, Clone, Copy)]
enum UserEvent {
    PtyBytesArrived,
    GestureEnded,
    MomentumTick,
    /// 1Hz heartbeat that keeps Uptime / Reqs / sub / team chrome
    /// fresh even when the PTY is silent.
    TickRedraw,
}

/// Entry point for the GPU UI. Performs the full anyclaude bootstrap
/// (config, debug logger, proxy server, teammate shim) and prepares
/// the spawn params for Claude Code, then hands off to the winit
/// event loop. The proxy + tokio runtime live in this function's
/// scope so they outlive the event loop and drop cleanly after
/// `event_loop.run_app` returns.
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
        .extend(crate::args::ArgAssembler::new().with_subagent_hooks(actual_addr.port()).build());

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

struct GpuApp {
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    scale_factor: f32,

    // Font system is owned at the app level — cosmic-text rasterizes
    // glyphs against it via the shape cache, and the swash cache holds
    // the bitmap data destined for the atlas.
    font_system: FontSystem,
    swash_cache: SwashCache,
    shape_cache: TextShapeCache,

    palette: AnsiPalette,
    cell_metrics: Option<CellMetrics>,
    /// Variable-width text cache for chrome (header / footer / popups).
    /// Separate from `shape_cache` because cache instances are family-
    /// scoped — terminal cells are Monospace, chrome is SansSerif.
    ui_shape_cache: TextShapeCache,

    // Lazily initialised in `resumed`: spawning the shell needs to know
    // the window's pixel size, which we don't have until then.
    pty: Option<ChildPty>,
    emulator: Option<Box<dyn TerminalEmulator>>,
    grid_size: (usize, usize),

    modifiers: ModifiersState,

    scroll: ScrollState,
    scroll_velocity: Option<ScrollVelocity>,
    momentum_abort: Option<AbortHandle>,
    gesture_end_abort: Option<AbortHandle>,
    /// 1Hz redraw heartbeat — see [`UserEvent::TickRedraw`]. Aborted
    /// implicitly when the proxy's send_event fails (window closed).
    periodic_tick_abort: Option<AbortHandle>,

    /// Current mouse cursor position in logical pixels (top-left
    /// origin). `None` before the first CursorMoved event.
    cursor_pos: Option<(f32, f32)>,
    /// True while the left mouse button is held with a selection in
    /// flight (a non-mouse-mode click → CursorMoved sequence).
    dragging_selection: bool,
    selection: Option<Selection>,
    last_click: Option<LastClick>,

    clipboard: Box<dyn Clipboard>,

    /// Session UUID. Generated at startup; shown in the header.
    /// Click on the "Session: …" text copies the full UUID to the
    /// clipboard with a green flash.
    session_id: String,
    /// `Instant` at process start. The header's "Uptime: <n>s" is
    /// derived from this.
    start_time: Instant,
    /// While `Some(deadline)`, the header renders "Session ID copied!"
    /// in the flash color instead of the dim grey session UUID.
    session_copied_until: Option<Instant>,
    /// X range of the session click hot-zone (logical pixels) in the
    /// header. Updated every redraw so the click handler can hit-test
    /// without recomputing the layout.
    session_click_zone: Option<(f32, f32)>,

    /// Live proxy backend state. Backend popup reads the list from
    /// here and Enter calls `switch_backend`; history popup pulls
    /// from `get_switch_log()`.
    backend_state: BackendState,
    /// Live subagent backend override. `None` means "use active backend
    /// for subagents". Header reads it for the `sub:` label; backend
    /// popup writes it via `Enter` in the Subagent section.
    subagent_backend: AgentBackendState,
    /// Live teammate backend override. Same shape as `subagent_backend`
    /// — separate field so subagents and teammates can route to
    /// different backends.
    teammate_backend: AgentBackendState,
    /// Proxy's observability hub. Header reads the total request
    /// counter via `snapshot()` once per frame.
    observability: ObservabilityHub,
    /// Spawn params for Claude Code, prepared by `run()` before the
    /// event loop. Used in `resumed()` to spawn the PTY child.
    spawn_command: String,
    spawn_args: Vec<String>,
    spawn_env: Vec<(String, String)>,
    /// Settings registry + current values. Persisted to disk on Cmd+E
    /// popup confirm (Enter). Loaded from Config at startup so the
    /// popup reflects the user's last choice.
    settings_manager: ClaudeSettingsManager,

    /// MVI stores for the popup overlays. At most one is `Visible` at
    /// a time; rendering and input routing check `is_visible()` on
    /// each. State machines + intent handling live in the respective
    /// `Actor` impls — this file is the wiring + render-side projection.
    backend_switch_store: Store<BackendSwitchActor>,
    history_store: Store<HistoryActor>,
    settings_store: Store<SettingsActor>,
}


#[derive(Debug, Clone, Copy)]
struct LastClick {
    time: Instant,
    point: CellPoint,
    count: u32,
}

impl GpuApp {
    fn new(
        proxy: EventLoopProxy<UserEvent>,
        spawn_command: String,
        spawn_args: Vec<String>,
        spawn_env: Vec<(String, String)>,
        backend_state: BackendState,
        subagent_backend: AgentBackendState,
        teammate_backend: AgentBackendState,
        observability: ObservabilityHub,
        settings_manager: ClaudeSettingsManager,
    ) -> Self {
        Self {
            proxy,
            window: None,
            renderer: None,
            scale_factor: 1.0,
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            shape_cache: TextShapeCache::with_family(FontFamily::Monospace),
            ui_shape_cache: TextShapeCache::with_family(FontFamily::SansSerif),
            palette: AnsiPalette::default_dark(),
            cell_metrics: None,
            pty: None,
            emulator: None,
            grid_size: (INITIAL_GRID_COLS, INITIAL_GRID_ROWS),
            modifiers: ModifiersState::empty(),
            scroll: ScrollState::default(),
            scroll_velocity: None,
            momentum_abort: None,
            gesture_end_abort: None,
            periodic_tick_abort: None,
            cursor_pos: None,
            dragging_selection: false,
            selection: None,
            last_click: None,
            clipboard: make_clipboard(),
            session_id: Uuid::new_v4().to_string(),
            start_time: Instant::now(),
            session_copied_until: None,
            session_click_zone: None,
            backend_state,
            subagent_backend,
            teammate_backend,
            observability,
            spawn_command,
            spawn_args,
            spawn_env,
            settings_manager,
            backend_switch_store: Store::new(BackendSwitchActor, |_effect| {}),
            history_store: Store::new(HistoryActor, |_effect| {}),
            settings_store: Store::new(SettingsActor, |_effect| {}),
        }
    }

    fn cell_metrics(&mut self) -> CellMetrics {
        if let Some(m) = self.cell_metrics {
            return m;
        }
        let metrics = measure_cell_metrics(
            &mut self.font_system,
            &mut self.shape_cache,
            FONT_SIZE,
            self.scale_factor,
        );
        self.cell_metrics = Some(metrics);
        metrics
    }

    /// The terminal area sits below the header chrome. Returns the
    /// rect (logical pixels, top-left origin) callers should pass to
    /// `populate_panel` / `build_cursor_rect` and use as the basis
    /// for mouse hit-testing.
    fn terminal_panel_rect(&self) -> PanelRect {
        let Some(window) = self.window.as_ref() else {
            return PanelRect::new(0.0, HEADER_HEIGHT_LOGICAL, 0.0, 0.0);
        };
        let size = window.inner_size();
        let sf = self.scale_factor.max(0.0001);
        let w_logical = size.width as f32 / sf;
        let h_logical = size.height as f32 / sf;
        let h = (h_logical - HEADER_HEIGHT_LOGICAL - FOOTER_HEIGHT_LOGICAL).max(0.0);
        PanelRect::new(0.0, HEADER_HEIGHT_LOGICAL, w_logical, h)
    }

    /// Compute the grid size (cols × rows) that fits inside the
    /// terminal panel rect at the current cell metrics. Both
    /// dimensions are clamped to at least 1 — a sub-cell terminal
    /// area is degenerate but should never panic.
    fn fit_grid(&mut self) -> (usize, usize) {
        let metrics = self.cell_metrics();
        let panel = self.terminal_panel_rect();
        let sf = self.scale_factor.max(0.0001);
        let cols = ((panel.w * sf / metrics.width_physical).floor() as usize).max(1);
        let rows = ((panel.h * sf / metrics.height_physical).floor() as usize).max(1);
        (cols, rows)
    }

    /// Resync emulator + PTY to the current window size. Called from
    /// `resumed` and on `Resized`/`ScaleFactorChanged`.
    fn sync_grid_to_window(&mut self) {
        let (cols, rows) = self.fit_grid();
        if self.grid_size == (cols, rows) {
            return;
        }
        self.grid_size = (cols, rows);
        if let Some(emu) = self.emulator.as_mut() {
            emu.resize(cols, rows);
        }
        if let Some(pty) = self.pty.as_ref() {
            pty.resize(cols as u16, rows as u16);
        }
    }

    /// Drain the PTY's pending bytes into the emulator. Returns true
    /// when at least one chunk arrived (caller should request redraw).
    /// Follow mode: if the scroll was at the bottom BEFORE applying
    /// the new bytes, re-pin to the bottom afterward so the cursor
    /// stays visible while the shell prints. Users who explicitly
    /// scrolled up keep position.
    fn drain_pty(&mut self) -> bool {
        let Some(pty) = self.pty.as_mut() else {
            return false;
        };
        let chunks = pty.drain();
        if chunks.is_empty() {
            return false;
        }
        self.refresh_scroll_geometry();
        let was_at_bottom = self.scroll.offset_y <= SCROLL_BOTTOM_EPSILON;
        if let Some(emu) = self.emulator.as_mut() {
            for chunk in chunks {
                emu.process(&chunk);
            }
        }
        self.refresh_scroll_geometry();
        if was_at_bottom {
            self.scroll.offset_y = 0.0;
        }
        true
    }

    /// Recompute the scroll bounds from the current emulator snapshot
    /// and window size. Called before any scroll mutation so clamping
    /// uses up-to-date geometry.
    fn refresh_scroll_geometry(&mut self) {
        let metrics = self.cell_metrics();
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Some(emu) = self.emulator.as_ref() else {
            return;
        };
        let sf = self.scale_factor.max(0.0001);
        let cell_h_logical = metrics.height_physical / sf;
        let snap = emu.snapshot();
        let visible_h_logical = window.inner_size().height as f32 / sf;
        self.scroll.total_size_px = snap.rows.len() as f32 * cell_h_logical;
        self.scroll.visible_px = visible_h_logical;
        let max = self.scroll.max_offset();
        if self.scroll.offset_y > max {
            self.scroll.offset_y = max;
        }
    }

    fn cancel_momentum(&mut self) {
        if let Some(a) = self.momentum_abort.take() {
            a.abort();
        }
    }

    fn cancel_gesture_end(&mut self) {
        if let Some(a) = self.gesture_end_abort.take() {
            a.abort();
        }
    }

    /// Apply a wheel delta. Trackpad `TouchPhase::Ended` kicks momentum
    /// immediately; for non-precise wheels (mice) a silence timeout
    /// falls back to the same path.
    fn on_wheel(&mut self, dy: f32, phase: TouchPhase, precise: bool) {
        // A new wheel event interrupts any in-flight momentum + pending kickoff.
        self.cancel_momentum();
        self.cancel_gesture_end();

        self.refresh_scroll_geometry();
        self.scroll.scroll_by(dy);
        self.scroll_velocity = Some(ScrollVelocity::record(
            self.scroll_velocity,
            Vec2::new(0.0, dy),
            Instant::now(),
        ));

        match phase {
            TouchPhase::Ended => {
                self.on_gesture_end();
            }
            TouchPhase::Cancelled => {
                self.scroll_velocity = None;
            }
            TouchPhase::Started | TouchPhase::Moved => {
                if !precise {
                    self.gesture_end_abort = Some(schedule_once(
                        self.proxy.clone(),
                        GESTURE_END_TIMEOUT,
                        UserEvent::GestureEnded,
                    ));
                }
            }
        }

        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    fn on_gesture_end(&mut self) {
        let Some(v) = self.scroll_velocity else { return };
        let speed = v.velocity.length();
        if speed < MOMENTUM_THRESHOLD {
            self.scroll_velocity = None;
            return;
        }
        self.scroll_velocity = Some(ScrollVelocity {
            velocity: v.clamped_for_momentum(),
            last_update: Instant::now(),
        });
        self.momentum_abort = Some(schedule_momentum_loop(
            self.proxy.clone(),
            MOMENTUM_FRAME_INTERVAL,
        ));
    }

    /// True when any popup MVI store is in its `Visible` state. Used
    /// to gate input routing and mouse-click priority.
    fn any_popup_visible(&self) -> bool {
        self.backend_switch_store.state().is_visible()
            || self.history_store.state().is_visible()
            || self.settings_store.state().is_visible()
    }

    /// Dispatch `Close` to every popup store. Called by Cmd+B / +H /
    /// +E before opening a new popup, by Esc, and by click-outside.
    fn close_all_popups(&mut self) {
        if self.backend_switch_store.state().is_visible() {
            self.backend_switch_store.dispatch(BackendSwitchIntent::Close);
        }
        if self.history_store.state().is_visible() {
            self.history_store.dispatch(HistoryIntent::Close);
        }
        if self.settings_store.state().is_visible() {
            self.settings_store.dispatch(SettingsIntent::Close);
        }
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Cmd+B handler — open or close the backend switch popup. Open
    /// dispatches the Open intent with the active backend pre-selected
    /// so pressing Enter is a no-op if the user is just inspecting.
    fn toggle_backend_switch_popup(&mut self) {
        if self.backend_switch_store.state().is_visible() {
            self.close_all_popups();
            return;
        }
        let cfg = self.backend_state.get_config();
        if cfg.backends.is_empty() {
            return;
        }
        let active = self.backend_state.get_active_backend();
        let backend_selection = cfg
            .backends
            .iter()
            .position(|b| b.name == active)
            .unwrap_or(0);
        // Close any other open popup first.
        self.close_all_popups();
        self.backend_switch_store
            .dispatch(BackendSwitchIntent::Open {
                backend_selection,
                subagent_selection: 0,
                teammate_selection: 0,
                backends_count: cfg.backends.len(),
            });
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Cmd+H handler — open or close the history popup. The switch
    /// log is snapshotted into the popup at open time; subsequent
    /// switches only show up after the user reopens.
    fn toggle_history_popup(&mut self) {
        if self.history_store.state().is_visible() {
            self.close_all_popups();
            return;
        }
        let entries = self.backend_state.get_switch_log();
        let mvi_entries: Vec<HistoryEntry> = entries
            .into_iter()
            .map(|e| HistoryEntry {
                timestamp: e.timestamp,
                from_backend: e.old_backend,
                to_backend: e.new_backend,
            })
            .collect();
        self.close_all_popups();
        self.history_store
            .dispatch(HistoryIntent::Load { entries: mvi_entries });
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Cmd+E handler — open or close the settings popup. Field
    /// snapshots are loaded from `settings_manager`; Space toggles
    /// rows (marks state dirty), Enter applies and saves, Esc
    /// discards.
    fn toggle_settings_popup(&mut self) {
        if self.settings_store.state().is_visible() {
            self.close_all_popups();
            return;
        }
        let fields: Vec<SettingsFieldSnapshot> = self
            .settings_manager
            .registry()
            .iter()
            .map(|def| SettingsFieldSnapshot {
                id: def.id,
                label: def.label,
                description: def.description,
                section: def.section,
                value: self.settings_manager.get(def.id),
            })
            .collect();
        if fields.is_empty() {
            return;
        }
        self.close_all_popups();
        self.settings_store
            .dispatch(SettingsIntent::Load { fields });
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Persist the settings popup's edits to disk. Reads the current
    /// MVI state, applies each row to the manager, then calls
    /// `save_claude_settings`. Errors are logged but non-fatal.
    fn apply_settings_and_save(&mut self) {
        let fields = match self.settings_store.state() {
            SettingsDialogState::Visible { fields, .. } => fields.clone(),
            SettingsDialogState::Hidden => return,
        };
        for field in &fields {
            self.settings_manager.set(field.id, field.value);
        }
        let snapshot = self
            .settings_manager
            .snapshot_values()
            .into_iter()
            .map(|(id, v)| (id.as_str().to_string(), v))
            .collect();
        if let Err(e) = save_claude_settings(&Config::config_path(), &snapshot) {
            eprintln!("anyclaude: failed to save settings: {e}");
        }
    }

    /// Route a keyboard event to the currently-open popup. Each store
    /// owns its own intent vocabulary; this method translates winit
    /// key events into the right dispatch. Esc is handled at the call
    /// site (close_all_popups). Enter triggers the popup's action
    /// (switch backend / save settings / dismiss history) and closes
    /// the popup.
    fn handle_popup_key(&mut self, event: &winit::event::KeyEvent) {
        if self.backend_switch_store.state().is_visible() {
            self.handle_backend_switch_key(event);
        } else if self.history_store.state().is_visible() {
            self.handle_history_key(event);
        } else if self.settings_store.state().is_visible() {
            self.handle_settings_key(event);
        }
    }

    fn handle_backend_switch_key(&mut self, event: &winit::event::KeyEvent) {
        match event.physical_key {
            PhysicalKey::Code(KeyCode::ArrowUp) => {
                self.backend_switch_store
                    .dispatch(BackendSwitchIntent::MoveUp);
                self.request_redraw();
            }
            PhysicalKey::Code(KeyCode::ArrowDown) => {
                self.backend_switch_store
                    .dispatch(BackendSwitchIntent::MoveDown);
                self.request_redraw();
            }
            PhysicalKey::Code(KeyCode::Tab) => {
                self.backend_switch_store
                    .dispatch(BackendSwitchIntent::NextSection);
                self.request_redraw();
            }
            PhysicalKey::Code(KeyCode::Enter) => {
                self.apply_backend_switch_selection();
                self.close_all_popups();
            }
            PhysicalKey::Code(KeyCode::Delete | KeyCode::Backspace) => {
                self.backend_switch_store
                    .dispatch(BackendSwitchIntent::Clear);
                self.request_redraw();
            }
            _ => {}
        }
    }

    /// Apply whichever action the active section maps to: the Active
    /// section calls `switch_backend`; the Subagent / Teammate sections
    /// write into their `AgentBackendState` (index 0 == Disabled
    /// → `None`, index N+1 == backend N). Errors are logged but
    /// non-fatal — the popup still closes.
    fn apply_backend_switch_selection(&mut self) {
        let (section, backend_sel, subagent_sel, teammate_sel) =
            match *self.backend_switch_store.state() {
                BackendSwitchState::Visible {
                    section,
                    backend_selection,
                    subagent_selection,
                    teammate_selection,
                    ..
                } => (
                    section,
                    backend_selection,
                    subagent_selection,
                    teammate_selection,
                ),
                BackendSwitchState::Hidden => return,
            };
        let cfg = self.backend_state.get_config();
        match section {
            BackendPopupSection::ActiveBackend => {
                if let Some(b) = cfg.backends.get(backend_sel) {
                    let id = b.name.clone();
                    if let Err(e) = self.backend_state.switch_backend(&id) {
                        eprintln!("anyclaude: backend switch failed: {e}");
                    }
                }
            }
            BackendPopupSection::SubagentBackend => {
                let new_value = override_selection_to_backend_id(&cfg.backends, subagent_sel);
                self.subagent_backend.set(new_value);
            }
            BackendPopupSection::TeammateBackend => {
                let new_value = override_selection_to_backend_id(&cfg.backends, teammate_sel);
                self.teammate_backend.set(new_value);
            }
        }
    }

    fn handle_history_key(&mut self, event: &winit::event::KeyEvent) {
        match event.physical_key {
            PhysicalKey::Code(KeyCode::ArrowUp) => {
                self.history_store.dispatch(HistoryIntent::ScrollUp);
                self.request_redraw();
            }
            PhysicalKey::Code(KeyCode::ArrowDown) => {
                self.history_store.dispatch(HistoryIntent::ScrollDown);
                self.request_redraw();
            }
            PhysicalKey::Code(KeyCode::Enter) => {
                self.close_all_popups();
            }
            _ => {}
        }
    }

    fn handle_settings_key(&mut self, event: &winit::event::KeyEvent) {
        match event.physical_key {
            PhysicalKey::Code(KeyCode::ArrowUp) => {
                self.settings_store.dispatch(SettingsIntent::MoveUp);
                self.request_redraw();
            }
            PhysicalKey::Code(KeyCode::ArrowDown) => {
                self.settings_store.dispatch(SettingsIntent::MoveDown);
                self.request_redraw();
            }
            PhysicalKey::Code(KeyCode::Space) => {
                self.settings_store.dispatch(SettingsIntent::Toggle);
                self.request_redraw();
            }
            PhysicalKey::Code(KeyCode::Enter) => {
                self.apply_settings_and_save();
                self.close_all_popups();
            }
            _ => {}
        }
    }

    fn request_redraw(&self) {
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Tear down the running Claude session and start a fresh one with
    /// the same spawn params. Wired to Cmd+R. The terminal state
    /// (emulator, scroll, selection) is reset so the new session
    /// renders into a clean panel.
    ///
    /// The old reader thread exits on its own as soon as its master
    /// PTY is dropped — the spawn flow is fire-and-forget.
    fn restart_pty(&mut self) {
        self.pty = None;
        let (cols, rows) = self.grid_size;
        self.emulator = Some(create_emulator(cols, rows, SCROLLBACK_LINES));
        self.scroll = ScrollState::default();
        self.scroll_velocity = None;
        self.cancel_momentum();
        self.cancel_gesture_end();
        self.selection = None;
        self.dragging_selection = false;
        self.last_click = None;

        let proxy = self.proxy.clone();
        match ChildPty::spawn(
            cols as u16,
            rows as u16,
            self.spawn_command.clone(),
            self.spawn_args.clone(),
            self.spawn_env.clone(),
            move || {
                let _ = proxy.send_event(UserEvent::PtyBytesArrived);
            },
        ) {
            Ok(pty) => {
                self.pty = Some(pty);
            }
            Err(e) => {
                eprintln!("anyclaude: failed to restart shell: {e}");
            }
        }
        self.request_redraw();
    }

    /// Copy the session UUID to the clipboard and trigger the
    /// header's "Session ID copied!" flash. Used by header click and
    /// the keyboard shortcut path (potentially later).
    fn copy_session_id(&mut self) {
        self.clipboard
            .write(ClipboardContent::plain_text(self.session_id.clone()));
        self.session_copied_until = Some(Instant::now() + SESSION_COPY_FLASH);
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Copy the current selection to the system clipboard. Mirrors
    /// term_grid: `selection_to_text` against the current emulator
    /// snapshot → `ClipboardContent::plain_text`. Empty selections are
    /// skipped silently.
    fn copy_selection(&mut self) {
        let Some(sel) = self.selection else { return };
        if sel.is_empty() {
            return;
        }
        let Some(emu) = self.emulator.as_ref() else { return };
        let snap = emu.snapshot();
        let text = selection_to_text(&sel, &snap);
        if text.is_empty() {
            return;
        }
        self.clipboard.write(ClipboardContent::plain_text(text));
    }

    /// Read the system clipboard and paste into the PTY. Mirrors
    /// Warp's `process_paste_event` step-for-step
    /// (`app/src/terminal/input.rs:10573`):
    ///
    ///   1. If `should_insert_text_on_paste(&content)` is true,
    ///      include `content.plain_text` in the payload.
    ///   2. Image filepaths in `content.paths` (filtered via
    ///      `get_image_filepaths_from_paths`) follow next — Claude
    ///      Code and other image-aware CLIs accept file paths as
    ///      input.
    ///   3. If `content.images` carries any pasteboard image data,
    ///      pick the highest-priority MIME from
    ///      `CLIPBOARD_IMAGE_MIME_TYPES`, save it to
    ///      `$TMPDIR/anyclaude_clipboard_<ts>.<ext>`, and append the
    ///      path to the payload.
    ///
    /// Paths are shell-quoted (single-quote escape) so spaces in
    /// names don't break tokenisation in the shell. The final
    /// payload is normalised (CRLF → LF) and wrapped in
    /// `\x1b[200~` … `\x1b[201~` when the emulator has bracketed
    /// paste enabled.
    fn paste_into_pty(&mut self) {
        let content = self.clipboard.read();
        let mut parts: Vec<String> = Vec::new();

        if should_insert_text_on_paste(&content) && !content.plain_text.is_empty() {
            parts.push(content.plain_text.clone());
        }

        if let Some(paths) = content.paths.as_deref() {
            for path in get_image_filepaths_from_paths(paths) {
                parts.push(shell_quote_path(&path));
            }
        }

        if let Some(images) = content.images.as_deref() {
            if let Some(best) = pick_best_image(images) {
                if let Some(path) = save_image_to_temp(best, "anyclaude_clipboard") {
                    parts.push(shell_quote_path(&path));
                }
            }
        }

        if parts.is_empty() {
            return;
        }
        let payload = parts.join(" ");
        let bracketed = self
            .emulator
            .as_ref()
            .map(|e| e.bracketed_paste())
            .unwrap_or(false);
        let bytes = encode_paste(&payload, bracketed);
        if let Some(pty) = self.pty.as_mut() {
            if let Err(e) = pty.write(&bytes) {
                eprintln!("anyclaude: paste write failed: {e}");
            }
        }
    }

    /// Translate a window-local logical-pixel position into the cell
    /// underneath. Inverse of `populate_panel`'s row positioning:
    ///   row_y_logical = row_idx * cell_h - baseline_offset + scroll_offset
    ///   row_idx       = (row_y_logical + baseline_offset - scroll_offset) / cell_h
    fn cell_at(&mut self, x: f32, y: f32) -> Option<CellPoint> {
        let metrics = self.cell_metrics();
        let panel = self.terminal_panel_rect();
        let emu = self.emulator.as_ref()?;
        let sf = self.scale_factor.max(0.0001);
        let cell_w_logical = metrics.width_physical / sf;
        let cell_h_logical = metrics.height_physical / sf;
        if cell_w_logical <= 0.0 || cell_h_logical <= 0.0 {
            return None;
        }
        // Mouse coords are window-relative; translate into the
        // terminal area so the row math matches `populate_panel`.
        let local_x = (x - panel.x).max(0.0);
        let local_y = (y - panel.y).max(0.0);
        let snap = emu.snapshot();
        let total_rows = snap.rows.len();
        let visible_rows = snap.visible_rows;
        let baseline_offset = total_rows.saturating_sub(visible_rows) as f32 * cell_h_logical;
        let row_unclamped =
            ((local_y + baseline_offset - self.scroll.offset_y) / cell_h_logical).floor();
        let row = row_unclamped.clamp(0.0, total_rows.saturating_sub(1) as f32) as usize;
        let cols = snap.rows.first().map(|r| r.cells.len()).unwrap_or(0);
        let col_unclamped = (local_x / cell_w_logical).floor();
        let col = col_unclamped.clamp(0.0, cols.saturating_sub(1) as f32) as usize;
        Some(CellPoint { row, col })
    }

    fn on_cursor_moved(&mut self, x: f32, y: f32) {
        self.cursor_pos = Some((x, y));
        if self.dragging_selection {
            if let Some(point) = self.cell_at(x, y) {
                if let Some(sel) = self.selection.as_mut() {
                    sel.cursor = point;
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
        }
    }

    fn on_mouse_press(&mut self) {
        let Some((x, y)) = self.cursor_pos else { return };
        // When a popup is open, a click anywhere dismisses it
        // (matching macOS modal-out behaviour) and is otherwise
        // swallowed — the click never starts a selection in the
        // terminal underneath.
        if self.any_popup_visible() {
            self.close_all_popups();
            return;
        }
        // Header click — copy session id to clipboard and flash the
        // label. Takes priority over selection so a header click
        // never lands inside the terminal area's coords.
        if y < HEADER_HEIGHT_LOGICAL {
            if let Some((sx, ex)) = self.session_click_zone {
                if x >= sx && x < ex {
                    self.copy_session_id();
                }
            }
            return;
        }
        // Apps in mouse-reporting mode (vim / htop / fzf) own the drag —
        // selection mustn't shadow them.
        let owns_mouse = self
            .emulator
            .as_ref()
            .map(|e| e.mouse_mode() != MouseMode::None)
            .unwrap_or(false);
        if owns_mouse {
            return;
        }
        let Some(point) = self.cell_at(x, y) else { return };
        let count = self.bump_click_count(point);
        let snap = self.emulator.as_ref().map(|e| e.snapshot());
        match count {
            1 => {
                self.selection = Some(Selection::new(point));
                self.dragging_selection = true;
            }
            2 => {
                let (start, end) = snap
                    .as_ref()
                    .map(|s| expand_word(point, s))
                    .unwrap_or((point, point));
                self.selection = Some(Selection {
                    anchor: start,
                    cursor: end,
                });
                // No drag after double-click; the user re-clicks to
                // start a linear selection.
                self.dragging_selection = false;
            }
            _ => {
                let (start, end) = snap
                    .as_ref()
                    .map(|s| expand_line(point, s))
                    .unwrap_or((point, point));
                self.selection = Some(Selection {
                    anchor: start,
                    cursor: end,
                });
                self.dragging_selection = false;
            }
        }
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    fn on_mouse_release(&mut self) {
        if self.dragging_selection {
            self.dragging_selection = false;
            // A click that didn't drag (anchor == cursor) clears the
            // selection — keeps "click somewhere to deselect" working.
            if self.selection.map(|s| s.is_empty()).unwrap_or(false) {
                self.selection = None;
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
        }
    }

    /// Update `self.last_click` based on the new press and return the
    /// 1..=3 click count. Resets to 1 when the click misses the
    /// previous cell or arrives after the threshold.
    fn bump_click_count(&mut self, point: CellPoint) -> u32 {
        let now = Instant::now();
        let new_count = match self.last_click {
            Some(lc)
                if lc.point == point
                    && now.duration_since(lc.time).as_millis() <= MULTI_CLICK_THRESHOLD_MS =>
            {
                if lc.count >= 3 {
                    1
                } else {
                    lc.count + 1
                }
            }
            _ => 1,
        };
        self.last_click = Some(LastClick {
            time: now,
            point,
            count: new_count,
        });
        new_count
    }

    fn on_momentum_tick(&mut self) {
        let Some(v) = self.scroll_velocity.as_mut() else { return };
        let now = Instant::now();
        let elapsed = now.duration_since(v.last_update).as_secs_f32();
        v.last_update = now;
        v.velocity = decay_velocity(v.velocity, elapsed);
        if v.velocity.length() < MOMENTUM_MIN_VELOCITY {
            self.cancel_momentum();
            self.scroll_velocity = None;
            return;
        }
        let delta = v.velocity * elapsed;
        self.refresh_scroll_geometry();
        self.scroll.scroll_by(delta.y);
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Render one frame: clear, populate cells, push cursor, draw
    /// header chrome, present.
    fn redraw(&mut self) {
        let metrics = self.cell_metrics();
        let panel = self.terminal_panel_rect();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Some(emulator) = self.emulator.as_ref() else {
            return;
        };
        let sf = self.scale_factor.max(0.0001);

        let snapshot = emulator.snapshot();
        let scroll_offset_y = self.scroll.offset_y;
        let mut rects: Vec<RectInstance> = Vec::new();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();
        populate_panel(
            &snapshot,
            panel,
            &self.palette,
            &mut self.font_system,
            &mut self.swash_cache,
            renderer.atlas_mut(),
            &mut self.shape_cache,
            FONT_SIZE,
            sf,
            metrics,
            scroll_offset_y,
            &mut rects,
            &mut glyphs,
        );
        if let Some(sel) = self.selection {
            push_selection_rects(
                &sel,
                &snapshot,
                panel,
                sf,
                metrics,
                scroll_offset_y,
                &mut rects,
            );
        }
        if let Some(cursor_rect) = build_cursor_rect(
            snapshot.cursor,
            snapshot.visible_start(),
            panel,
            sf,
            metrics,
            scroll_offset_y,
        ) {
            rects.push(cursor_rect);
        }

        // Expire the session-copied flash if its deadline passed so
        // future frames skip the active-color branch in draw_header.
        if let Some(deadline) = self.session_copied_until {
            if Instant::now() >= deadline {
                self.session_copied_until = None;
            }
        }
        let active_backend = self.backend_state.get_active_backend();
        let cfg = self.backend_state.get_config();
        let resolve_display = |id: &str| -> Option<String> {
            cfg.backends
                .iter()
                .find(|b| b.name == id)
                .map(|b| b.display_name.clone())
        };
        let subagent_label = self
            .subagent_backend
            .get()
            .and_then(|id| resolve_display(&id));
        let teammate_label = self
            .teammate_backend
            .get()
            .and_then(|id| resolve_display(&id));
        let total_reqs: u64 = self
            .observability
            .snapshot()
            .per_backend
            .values()
            .map(|m| m.total)
            .sum();
        self.session_click_zone = draw_header(
            renderer.atlas_mut(),
            &mut self.font_system,
            &mut self.swash_cache,
            &mut self.ui_shape_cache,
            &mut glyphs,
            &active_backend,
            subagent_label.as_deref(),
            teammate_label.as_deref(),
            total_reqs,
            &self.session_id,
            self.start_time,
            self.session_copied_until.is_some(),
            sf,
        );

        let window_size = window.inner_size();
        let window_w_logical = window_size.width as f32 / sf;
        let window_h_logical = window_size.height as f32 / sf;
        draw_footer(
            renderer.atlas_mut(),
            &mut self.font_system,
            &mut self.swash_cache,
            &mut self.ui_shape_cache,
            &mut glyphs,
            window_w_logical,
            window_h_logical,
            sf,
        );

        // Build an optional overlay layer for whichever popup MVI
        // store is currently `Visible`. At most one popup is shown at
        // a time (close_all_popups before opening) so the dispatch
        // is sequential — first match wins.
        let mut overlay_shadows: Vec<term_gpu::ShadowInstance> = Vec::new();
        let mut overlay_rects: Vec<RectInstance> = Vec::new();
        let mut overlay_glyphs: Vec<GlyphInstance> = Vec::new();
        let backend_state_visible = self.backend_switch_store.state().is_visible();
        let history_state_visible = self.history_store.state().is_visible();
        let settings_state_visible = self.settings_store.state().is_visible();
        if backend_state_visible {
            let items_and_ids: Vec<(String, String)> = self
                .backend_state
                .get_config()
                .backends
                .iter()
                .map(|b| (b.display_name.clone(), b.name.clone()))
                .collect();
            let active_backend = self.backend_state.get_active_backend();
            let current_subagent = self.subagent_backend.get();
            let current_teammate = self.teammate_backend.get();
            draw_backend_switch_popup(
                self.backend_switch_store.state(),
                &items_and_ids,
                &active_backend,
                current_subagent.as_deref(),
                current_teammate.as_deref(),
                renderer.atlas_mut(),
                &mut self.font_system,
                &mut self.swash_cache,
                &mut self.ui_shape_cache,
                &mut overlay_shadows,
                &mut overlay_rects,
                &mut overlay_glyphs,
                window_w_logical,
                window_h_logical,
                sf,
            );
        } else if history_state_visible {
            draw_history_popup(
                self.history_store.state(),
                renderer.atlas_mut(),
                &mut self.font_system,
                &mut self.swash_cache,
                &mut self.ui_shape_cache,
                &mut overlay_shadows,
                &mut overlay_rects,
                &mut overlay_glyphs,
                window_w_logical,
                window_h_logical,
                sf,
            );
        } else if settings_state_visible {
            draw_settings_popup(
                self.settings_store.state(),
                renderer.atlas_mut(),
                &mut self.font_system,
                &mut self.swash_cache,
                &mut self.ui_shape_cache,
                &mut overlay_shadows,
                &mut overlay_rects,
                &mut overlay_glyphs,
                window_w_logical,
                window_h_logical,
                sf,
            );
        }
        let overlay = if overlay_shadows.is_empty()
            && overlay_rects.is_empty()
            && overlay_glyphs.is_empty()
        {
            None
        } else {
            Some(RenderLayer {
                shadows: &overlay_shadows,
                rects: &overlay_rects,
                glyphs: &overlay_glyphs,
            })
        };

        window.pre_present_notify();
        renderer.render(
            RenderLayer::rects_and_glyphs(&rects, &glyphs),
            overlay,
            0.0,
        );
        self.shape_cache.end_frame();
        self.ui_shape_cache.end_frame();
    }
}

/// Draw the top header chrome: dim grey labels for backend / sub /
/// team / Reqs / Uptime / Session, separated by " │ ". The Session
/// label is the click-to-copy hot-zone; the returned `(start_x, end_x)`
/// goes into `GpuApp::session_click_zone` for the mouse handler.
///
/// Free function (not method) so the caller can hold a `&mut renderer`
/// borrow across the call — `&mut self` here would collide.
#[allow(clippy::too_many_arguments)]
fn draw_header(
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    glyphs: &mut Vec<GlyphInstance>,
    active_backend: &str,
    subagent: Option<&str>,
    teammate: Option<&str>,
    reqs: u64,
    session_id: &str,
    start_time: Instant,
    session_copied_active: bool,
    sf: f32,
) -> Option<(f32, f32)> {
    let backend = active_backend;
    let sub = subagent.unwrap_or("—");
    let team = teammate.unwrap_or("—");
    let uptime_s = start_time.elapsed().as_secs();

    let sep = " │ ";
    let baseline_y = HEADER_HEIGHT_LOGICAL * 0.7;
    let mut x = 8.0;

    let segments: [String; 5] = [
        format!("backend: {backend}"),
        format!("sub: {sub}"),
        format!("team: {team}"),
        format!("Reqs: {reqs}"),
        format!("Uptime: {uptime_s}s"),
    ];
    for seg in &segments {
        x = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            seg,
            x,
            baseline_y,
            CHROME_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            CHROME_TEXT_COLOR,
        );
        x = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            sep,
            x,
            baseline_y,
            CHROME_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            CHROME_TEXT_COLOR,
        );
    }

    let session_text = if session_copied_active {
        "Session ID copied!".to_string()
    } else {
        format!("Session: {session_id}")
    };
    let session_color = if session_copied_active {
        CHROME_FLASH_COLOR
    } else {
        CHROME_TEXT_COLOR
    };
    let session_start_x = x;
    let session_end_x = push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        &session_text,
        x,
        baseline_y,
        CHROME_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        session_color,
    );
    Some((session_start_x, session_end_x))
}

/// Draw the bottom footer chrome: hotkey hints at the left edge,
/// version string right-aligned. Free function for the same reason
/// `draw_header` is — keeps the `&mut renderer` borrow viable in
/// the caller.
#[allow(clippy::too_many_arguments)]
fn draw_footer(
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    glyphs: &mut Vec<GlyphInstance>,
    window_w_logical: f32,
    window_h_logical: f32,
    sf: f32,
) {
    let baseline_y = window_h_logical - FOOTER_HEIGHT_LOGICAL * 0.3;

    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        FOOTER_HINTS,
        0.0,
        baseline_y,
        CHROME_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );

    let version_text = format!("v{APP_VERSION} ");
    let version_w = measure_label_width(
        font_system,
        ui_shape_cache,
        &version_text,
        CHROME_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
    );
    let version_x = (window_w_logical - version_w).max(0.0);
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        &version_text,
        version_x,
        baseline_y,
        CHROME_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
}

impl ApplicationHandler<UserEvent> for GpuApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("anyclaude")
            .with_inner_size(LogicalSize::new(INITIAL_W, INITIAL_H));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("anyclaude: failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let renderer = GpuRenderer::new(window.clone());
        self.scale_factor = renderer.scale_factor();
        self.window = Some(window.clone());
        self.renderer = Some(renderer);

        let (cols, rows) = self.fit_grid();
        self.grid_size = (cols, rows);
        self.emulator = Some(create_emulator(cols, rows, SCROLLBACK_LINES));

        let proxy = self.proxy.clone();
        match ChildPty::spawn(
            cols as u16,
            rows as u16,
            self.spawn_command.clone(),
            self.spawn_args.clone(),
            self.spawn_env.clone(),
            move || {
                let _ = proxy.send_event(UserEvent::PtyBytesArrived);
            },
        ) {
            Ok(pty) => {
                self.pty = Some(pty);
            }
            Err(e) => {
                eprintln!("anyclaude: failed to spawn shell: {e}");
                event_loop.exit();
                return;
            }
        }

        self.periodic_tick_abort = Some(schedule_periodic_redraw(self.proxy.clone()));

        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyBytesArrived => {
                if self.drain_pty() {
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
            UserEvent::GestureEnded => {
                self.on_gesture_end();
            }
            UserEvent::MomentumTick => {
                self.on_momentum_tick();
            }
            UserEvent::TickRedraw => {
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(new_size);
                }
                self.sync_grid_to_window();
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                if let Some(r) = self.renderer.as_mut() {
                    r.set_scale_factor(self.scale_factor);
                }
                // Cell metrics depend on scale_factor; invalidate and
                // resync grid to the new physical cell size.
                self.cell_metrics = None;
                self.sync_grid_to_window();
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.update_modifiers(mods);
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let (precise, dy) = match delta {
                    MouseScrollDelta::PixelDelta(p) => (true, p.y as f32),
                    MouseScrollDelta::LineDelta(_, v) => (false, v * NUM_PIXELS_PER_LINE),
                };
                self.on_wheel(dy, phase, precise);
            }
            WindowEvent::CursorMoved { position, .. } => {
                let PhysicalPosition { x, y } = position;
                let sf = self.scale_factor.max(0.0001);
                self.on_cursor_moved(x as f32 / sf, y as f32 / sf);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.on_mouse_press(),
                ElementState::Released => self.on_mouse_release(),
            },
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed =>
            {
                // Popups own keyboard input while open: navigation,
                // selection, dismiss. Everything else (shell control
                // codes, app shortcuts) is suppressed.
                if self.any_popup_visible() {
                    if let PhysicalKey::Code(KeyCode::Escape) = event.physical_key {
                        self.close_all_popups();
                    } else {
                        self.handle_popup_key(&event);
                    }
                    return;
                }
                // Cmd/Super combos are app-level shortcuts (clipboard,
                // quit, popups). Match on physical_key, not logical_key,
                // so they work on every keyboard layout: Cmd+C on a
                // Russian / French / Greek layout would otherwise see
                // `Key::Character("с"|"ç"|"ψ")` and miss the match.
                if self.modifiers.super_key() {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        match code {
                            KeyCode::KeyC => self.copy_selection(),
                            KeyCode::KeyV => self.paste_into_pty(),
                            KeyCode::KeyB => self.toggle_backend_switch_popup(),
                            KeyCode::KeyH => self.toggle_history_popup(),
                            KeyCode::KeyE => self.toggle_settings_popup(),
                            KeyCode::KeyR => self.restart_pty(),
                            KeyCode::KeyQ => event_loop.exit(),
                            _ => {}
                        }
                    }
                    return;
                }
                // Ctrl combos belong to the shell (Ctrl+C / Ctrl+D /
                // ...) and pass straight through encode_key.
                let Some(bytes) = encode_key(&event.logical_key, self.modifiers) else {
                    return;
                };
                if let Some(pty) = self.pty.as_mut() {
                    if let Err(e) = pty.write(&bytes) {
                        eprintln!("anyclaude: PTY write failed: {e}");
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }
}

impl GpuApp {
    fn update_modifiers(&mut self, mods: Modifiers) {
        self.modifiers = mods.state();
    }
}

/// Spawn a one-shot abortable timer that sends `event` after `delay`.
/// Used to fall back to `GestureEnded` after a silence timeout when
/// the input device doesn't emit `TouchPhase::Ended` (mice).
fn schedule_once(
    proxy: EventLoopProxy<UserEvent>,
    delay: Duration,
    event: UserEvent,
) -> AbortHandle {
    let (fut, abort) = abortable(async move {
        Delay::new(delay).await;
        let _ = proxy.send_event(event);
    });
    std::thread::spawn(move || {
        let _ = futures::executor::block_on(fut);
    });
    abort
}

/// Spawn an abortable loop that fires `TickRedraw` once per second so
/// header chrome (Uptime / Reqs / sub / team) refreshes even when the
/// PTY is idle. The loop exits the moment `send_event` fails — the
/// usual "window dropped, event loop gone" path.
fn schedule_periodic_redraw(proxy: EventLoopProxy<UserEvent>) -> AbortHandle {
    let (fut, abort) = abortable(async move {
        loop {
            Delay::new(Duration::from_secs(1)).await;
            if proxy.send_event(UserEvent::TickRedraw).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let _ = futures::executor::block_on(fut);
    });
    abort
}

/// Spawn an abortable loop that fires `MomentumTick` every `interval`
/// until aborted or the receiver is gone.
fn schedule_momentum_loop(proxy: EventLoopProxy<UserEvent>, interval: Duration) -> AbortHandle {
    let (fut, abort) = abortable(async move {
        loop {
            Delay::new(interval).await;
            if proxy.send_event(UserEvent::MomentumTick).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let _ = futures::executor::block_on(fut);
    });
    abort
}

/// Map an override-section selection index into the backend id it
/// represents. Index 0 is the "Disabled" leader (returns `None`);
/// indices 1..=N map to `backends[i - 1]`. Out-of-range indices fall
/// back to `None` so a stale state never panics.
fn override_selection_to_backend_id(backends: &[Backend], selection: usize) -> Option<String> {
    if selection == 0 {
        return None;
    }
    backends.get(selection - 1).map(|b| b.name.clone())
}

/// Backend popup with three independent sections — Active, Subagent,
/// Teammate. Tab cycles the active section; Up/Down move selection
/// within the active section. Enter applies the section's action;
/// Del/Backspace resets the override sections back to Disabled.
///
/// Mirrors the legacy ratatui chrome (`backend_switch::dialog`) layout
/// 1:1 so users coming from the old UI find the same affordances.
#[allow(clippy::too_many_arguments)]
fn draw_backend_switch_popup(
    state: &BackendSwitchState,
    items_and_ids: &[(String, String)],
    active_backend: &str,
    current_subagent: Option<&str>,
    current_teammate: Option<&str>,
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    shadows: &mut Vec<term_gpu::ShadowInstance>,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    window_w: f32,
    window_h: f32,
    sf: f32,
) {
    let (active_section, backend_sel, subagent_sel, teammate_sel) = match *state {
        BackendSwitchState::Visible {
            section,
            backend_selection,
            subagent_selection,
            teammate_selection,
            ..
        } => (
            section,
            backend_selection,
            subagent_selection,
            teammate_selection,
        ),
        BackendSwitchState::Hidden => return,
    };
    let n = items_and_ids.len();
    if n == 0 {
        return;
    }

    let title = "Select Backend";
    let footer_hint =
        "Tab: Section  ↑/↓: Move  Enter: Select  Del: Clear  Esc: Close";
    let separator = "  ──────────────────";

    let measure = |fs: &mut FontSystem,
                   sc: &mut TextShapeCache,
                   s: &str,
                   weight: Weight,
                   style: Style|
     -> f32 { measure_label_width(fs, sc, s, POPUP_FONT_SIZE, sf, weight, style) };

    // Width: longest of title / footer / headers / longest formatted item.
    let mut content_w = measure(
        font_system,
        ui_shape_cache,
        title,
        Weight::BOLD,
        Style::Normal,
    );
    content_w = content_w.max(measure(
        font_system,
        ui_shape_cache,
        footer_hint,
        Weight::NORMAL,
        Style::Normal,
    ));
    for header in &[
        "▸ Active Backend",
        "▸ Subagent Backend",
        "▸ Teammate Backend",
    ] {
        content_w = content_w.max(measure(
            font_system,
            ui_shape_cache,
            header,
            Weight::BOLD,
            Style::Normal,
        ));
    }
    // Longest possible item row: "  → 10. <name>  [Selected]".
    let max_name_w = items_and_ids
        .iter()
        .map(|(name, _)| {
            measure(
                font_system,
                ui_shape_cache,
                name,
                Weight::NORMAL,
                Style::Normal,
            )
        })
        .fold(0.0_f32, f32::max);
    let prefix_w = measure(
        font_system,
        ui_shape_cache,
        "  → 10. ",
        Weight::NORMAL,
        Style::Normal,
    );
    let suffix_w = measure(
        font_system,
        ui_shape_cache,
        "  [Selected]",
        Weight::NORMAL,
        Style::Normal,
    );
    content_w = content_w.max(prefix_w + max_name_w + suffix_w);
    let disabled_w = measure(
        font_system,
        ui_shape_cache,
        "    Disabled (use active backend)  [Active]",
        Weight::NORMAL,
        Style::Normal,
    );
    content_w = content_w.max(disabled_w);

    let width = (content_w + POPUP_PADDING * 2.0).max(POPUP_MIN_WIDTH);

    // Total rows: title + gap + (header + sep + items) per section + gaps + footer.
    let active_rows = n as f32;
    let override_rows = (n + 1) as f32;
    let total_rows: f32 = 1.0  // title
        + 1.0                  // gap before sections
        + 1.0 + 1.0 + active_rows   // Active section
        + 1.0                  // gap
        + 1.0 + 1.0 + override_rows // Subagent
        + 1.0                  // gap
        + 1.0 + 1.0 + override_rows // Teammate
        + 1.0                  // gap before footer
        + 1.0; // footer
    let height = POPUP_PADDING * 2.0 + total_rows * POPUP_LINE_HEIGHT;

    let x = ((window_w - width) * 0.5).max(0.0);
    let y = ((window_h - height) * 0.25).max(0.0);

    draw_popup_chrome(x, y, width, height, shadows, rects);

    let content_x = x + POPUP_PADDING;
    let mut cursor_y = y + POPUP_PADDING + POPUP_LINE_HEIGHT * 0.75;

    // Title.
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        title,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::BOLD,
        Style::Normal,
        DEFAULT_FG_FOR_POPUP_SELECTED,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    cursor_y += POPUP_LINE_HEIGHT;

    // --- Active Backend section ---
    push_section_header(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        "Active Backend",
        active_section == BackendPopupSection::ActiveBackend,
        content_x,
        cursor_y,
        sf,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        separator,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    for (idx, (name, id)) in items_and_ids.iter().enumerate() {
        let in_section = active_section == BackendPopupSection::ActiveBackend;
        let selected = in_section && idx == backend_sel;
        let status = if id == active_backend {
            Some(("Active", CHROME_FLASH_COLOR))
        } else {
            None
        };
        push_backend_item(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            rects,
            x,
            width,
            content_x,
            cursor_y,
            sf,
            idx + 1,
            name,
            status,
            selected,
        );
        cursor_y += POPUP_LINE_HEIGHT;
    }
    cursor_y += POPUP_LINE_HEIGHT;

    // --- Subagent Backend section ---
    push_section_header(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        "Subagent Backend",
        active_section == BackendPopupSection::SubagentBackend,
        content_x,
        cursor_y,
        sf,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        separator,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    cursor_y += push_override_section_rows(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        rects,
        items_and_ids,
        current_subagent,
        subagent_sel,
        active_section == BackendPopupSection::SubagentBackend,
        x,
        width,
        content_x,
        cursor_y,
        sf,
    );
    cursor_y += POPUP_LINE_HEIGHT;

    // --- Teammate Backend section ---
    push_section_header(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        "Teammate Backend",
        active_section == BackendPopupSection::TeammateBackend,
        content_x,
        cursor_y,
        sf,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        separator,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    cursor_y += push_override_section_rows(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        rects,
        items_and_ids,
        current_teammate,
        teammate_sel,
        active_section == BackendPopupSection::TeammateBackend,
        x,
        width,
        content_x,
        cursor_y,
        sf,
    );
    cursor_y += POPUP_LINE_HEIGHT;

    // --- Footer hint ---
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        footer_hint,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
}

/// Draw a section header line prefixed with `▸` when this section is
/// the active one (Tab target) and two spaces otherwise.
#[allow(clippy::too_many_arguments)]
fn push_section_header(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    ui_shape_cache: &mut TextShapeCache,
    glyphs: &mut Vec<GlyphInstance>,
    label: &str,
    is_active: bool,
    content_x: f32,
    cursor_y: f32,
    sf: f32,
) {
    let prefix = if is_active { "▸ " } else { "  " };
    let line = format!("{prefix}{label}");
    let color = if is_active {
        DEFAULT_FG_FOR_POPUP_SELECTED
    } else {
        CHROME_TEXT_COLOR
    };
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        &line,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::BOLD,
        Style::Normal,
        color,
    );
}

/// Draw a single backend item row: optional highlight bar, numbered
/// prefix, display name, optional `[status]` suffix in `status_color`.
#[allow(clippy::too_many_arguments)]
fn push_backend_item(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    ui_shape_cache: &mut TextShapeCache,
    glyphs: &mut Vec<GlyphInstance>,
    rects: &mut Vec<RectInstance>,
    popup_x: f32,
    popup_w: f32,
    content_x: f32,
    cursor_y: f32,
    sf: f32,
    item_index: usize,
    display_name: &str,
    status: Option<(&str, [f32; 4])>,
    is_selected: bool,
) {
    if is_selected {
        rects.push(RectInstance {
            pos: [popup_x + 1.0, cursor_y - POPUP_LINE_HEIGHT * 0.75],
            size: [popup_w - 2.0, POPUP_LINE_HEIGHT],
            color: POPUP_HIGHLIGHT_COLOR,
        });
    }
    let prefix = if is_selected {
        format!("  → {}. ", item_index)
    } else {
        format!("    {}. ", item_index)
    };
    let row_color = if is_selected {
        DEFAULT_FG_FOR_POPUP_SELECTED
    } else {
        CHROME_TEXT_COLOR
    };
    let mut x_cursor = push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        &prefix,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        row_color,
    );
    x_cursor = push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        display_name,
        x_cursor,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        row_color,
    );
    if let Some((status_text, color)) = status {
        x_cursor = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            "  [",
            x_cursor,
            cursor_y,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            row_color,
        );
        x_cursor = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            status_text,
            x_cursor,
            cursor_y,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            color,
        );
        let _ = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            "]",
            x_cursor,
            cursor_y,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            row_color,
        );
    }
}

/// Draw the body of a Subagent / Teammate section: a "Disabled" row
/// first (selection index 0), then each backend with a `[Selected]`
/// status when it matches the override's current value. Returns the
/// total vertical advance (rows × `POPUP_LINE_HEIGHT`) so the caller
/// can position whatever follows.
#[allow(clippy::too_many_arguments)]
fn push_override_section_rows(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    ui_shape_cache: &mut TextShapeCache,
    glyphs: &mut Vec<GlyphInstance>,
    rects: &mut Vec<RectInstance>,
    items_and_ids: &[(String, String)],
    current_id: Option<&str>,
    selection: usize,
    in_section: bool,
    popup_x: f32,
    popup_w: f32,
    content_x: f32,
    cursor_y: f32,
    sf: f32,
) -> f32 {
    let mut local_y = cursor_y;

    // Row 0 — Disabled.
    let disabled_selected = in_section && selection == 0;
    if disabled_selected {
        rects.push(RectInstance {
            pos: [popup_x + 1.0, local_y - POPUP_LINE_HEIGHT * 0.75],
            size: [popup_w - 2.0, POPUP_LINE_HEIGHT],
            color: POPUP_HIGHLIGHT_COLOR,
        });
    }
    let disabled_color = if disabled_selected {
        DEFAULT_FG_FOR_POPUP_SELECTED
    } else {
        CHROME_TEXT_COLOR
    };
    let prefix = if disabled_selected { "  → " } else { "    " };
    let mut x_cursor = push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        prefix,
        content_x,
        local_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        disabled_color,
    );
    x_cursor = push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        "Disabled (use active backend)",
        x_cursor,
        local_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        disabled_color,
    );
    if current_id.is_none() {
        let _ = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            "  [Active]",
            x_cursor,
            local_y,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            CHROME_FLASH_COLOR,
        );
    }
    local_y += POPUP_LINE_HEIGHT;

    // Rows 1..=n — backends, idx+1 in the override selection space.
    for (idx, (name, id)) in items_and_ids.iter().enumerate() {
        let selection_idx = idx + 1;
        let selected = in_section && selection == selection_idx;
        let status = if current_id == Some(id.as_str()) {
            Some(("Selected", CHROME_FLASH_COLOR))
        } else {
            None
        };
        push_backend_item(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            rects,
            popup_x,
            popup_w,
            content_x,
            local_y,
            sf,
            idx + 1,
            name,
            status,
            selected,
        );
        local_y += POPUP_LINE_HEIGHT;
    }

    local_y - cursor_y
}

#[allow(clippy::too_many_arguments)]
fn draw_history_popup(
    state: &HistoryDialogState,
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    shadows: &mut Vec<term_gpu::ShadowInstance>,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    window_w: f32,
    window_h: f32,
    sf: f32,
) {
    let (entries, scroll_offset) = match state {
        HistoryDialogState::Visible {
            entries,
            scroll_offset,
        } => (entries, *scroll_offset),
        HistoryDialogState::Hidden => return,
    };
    let items: Vec<String> = entries
        .iter()
        .rev()
        .map(|e| {
            let secs = e
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let from = e.from_backend.as_deref().unwrap_or("(initial)");
            format!("{secs}  ·  {from}  →  {}", e.to_backend)
        })
        .collect();
    draw_string_list_popup(
        "History",
        &items,
        scroll_offset.min(items.len().saturating_sub(1)),
        Some("(no history yet)"),
        atlas,
        font_system,
        swash_cache,
        ui_shape_cache,
        shadows,
        rects,
        glyphs,
        window_w,
        window_h,
        sf,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_settings_popup(
    state: &SettingsDialogState,
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    shadows: &mut Vec<term_gpu::ShadowInstance>,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    window_w: f32,
    window_h: f32,
    sf: f32,
) {
    let (fields, focused) = match state {
        SettingsDialogState::Visible {
            fields, focused, ..
        } => (fields, *focused),
        SettingsDialogState::Hidden => return,
    };
    // Format each row as "[x] Label" / "[ ] Label" — checkbox is a
    // glyph prefix; the focus highlight comes from
    // draw_string_list_popup's selection bar.
    let formatted: Vec<String> = fields
        .iter()
        .map(|f| {
            let mark = if f.value { "[x]" } else { "[ ]" };
            format!("{mark}  {}", f.label)
        })
        .collect();
    draw_string_list_popup(
        "Settings  ·  Space toggle · Enter save · Esc cancel",
        &formatted,
        focused,
        None,
        atlas,
        font_system,
        swash_cache,
        ui_shape_cache,
        shadows,
        rects,
        glyphs,
        window_w,
        window_h,
        sf,
    );
}

/// Render a centered popup whose body is a list of strings with one
/// row visually selected. Used by both the backend-switch popup
/// (actionable selection) and the history popup (read-only browse).
/// When `items` is empty and an `empty_placeholder` is supplied, the
/// placeholder renders in dim grey instead of an item list.
#[allow(clippy::too_many_arguments)]
fn draw_string_list_popup(
    title: &str,
    items: &[String],
    selected: usize,
    empty_placeholder: Option<&str>,
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    shadows: &mut Vec<term_gpu::ShadowInstance>,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    window_w: f32,
    window_h: f32,
    sf: f32,
) {
    // Width: max of title / item / placeholder widths plus padding,
    // clamped to a sane minimum so a short list doesn't render as a
    // thin sliver.
    let title_w = measure_label_width(
        font_system,
        ui_shape_cache,
        title,
        POPUP_FONT_SIZE,
        sf,
        Weight::BOLD,
        Style::Normal,
    );
    let mut content_w = title_w;
    for item in items {
        let w = measure_label_width(
            font_system,
            ui_shape_cache,
            item,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
        );
        if w > content_w {
            content_w = w;
        }
    }
    if items.is_empty() {
        if let Some(p) = empty_placeholder {
            let w = measure_label_width(
                font_system,
                ui_shape_cache,
                p,
                POPUP_FONT_SIZE,
                sf,
                Weight::NORMAL,
                Style::Italic,
            );
            if w > content_w {
                content_w = w;
            }
        }
    }
    let width = (content_w + POPUP_PADDING * 2.0).max(POPUP_MIN_WIDTH);
    let body_rows = if items.is_empty() { 1.0 } else { items.len() as f32 };
    let height = POPUP_PADDING * 2.0
        + POPUP_LINE_HEIGHT          // title row
        + POPUP_LINE_HEIGHT * 0.5    // gap
        + body_rows * POPUP_LINE_HEIGHT;

    let x = ((window_w - width) * 0.5).max(0.0);
    let y = ((window_h - height) * 0.4).max(0.0);

    draw_popup_chrome(x, y, width, height, shadows, rects);

    let content_x = x + POPUP_PADDING;
    let mut cursor_y = y + POPUP_PADDING + POPUP_LINE_HEIGHT * 0.75;
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        title,
        content_x,
        cursor_y,
        POPUP_FONT_SIZE,
        sf,
        Weight::BOLD,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
    cursor_y += POPUP_LINE_HEIGHT;
    cursor_y += POPUP_LINE_HEIGHT * 0.5;
    let item_top_y = cursor_y - POPUP_LINE_HEIGHT * 0.75;

    if items.is_empty() {
        if let Some(p) = empty_placeholder {
            push_label(
                font_system,
                swash_cache,
                atlas,
                ui_shape_cache,
                glyphs,
                p,
                content_x,
                cursor_y,
                POPUP_FONT_SIZE,
                sf,
                Weight::NORMAL,
                Style::Italic,
                CHROME_TEXT_COLOR,
            );
        }
        return;
    }

    let highlight_y = item_top_y + (selected as f32) * POPUP_LINE_HEIGHT;
    rects.push(RectInstance {
        pos: [x + 1.0, highlight_y],
        size: [width - 2.0, POPUP_LINE_HEIGHT],
        color: POPUP_HIGHLIGHT_COLOR,
    });

    for (idx, item) in items.iter().enumerate() {
        let color = if idx == selected {
            DEFAULT_FG_FOR_POPUP_SELECTED
        } else {
            CHROME_TEXT_COLOR
        };
        push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            item,
            content_x,
            cursor_y,
            POPUP_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            color,
        );
        cursor_y += POPUP_LINE_HEIGHT;
    }
}

/// Push the shadow + background + 1px-border frame for any popup
/// rectangle.
fn draw_popup_chrome(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    shadows: &mut Vec<term_gpu::ShadowInstance>,
    rects: &mut Vec<RectInstance>,
) {
    shadows.push(term_gpu::ShadowInstance {
        pos: [x, y],
        size: [width, height],
        blur_radius: POPUP_SHADOW_BLUR,
        corner_radius: POPUP_CORNER_RADIUS,
        offset: [0.0, POPUP_SHADOW_OFFSET_Y],
        color: POPUP_SHADOW_COLOR,
    });
    rects.push(RectInstance {
        pos: [x, y],
        size: [width, height],
        color: POPUP_BG_COLOR,
    });
    rects.push(RectInstance {
        pos: [x, y],
        size: [width, 1.0],
        color: POPUP_BORDER_COLOR,
    });
    rects.push(RectInstance {
        pos: [x, y + height - 1.0],
        size: [width, 1.0],
        color: POPUP_BORDER_COLOR,
    });
    rects.push(RectInstance {
        pos: [x, y],
        size: [1.0, height],
        color: POPUP_BORDER_COLOR,
    });
    rects.push(RectInstance {
        pos: [x + width - 1.0, y],
        size: [1.0, height],
        color: POPUP_BORDER_COLOR,
    });
}

/// Brighter foreground for the selected popup item to contrast with
/// the highlight bar.
const DEFAULT_FG_FOR_POPUP_SELECTED: [f32; 4] = [0.95, 0.95, 0.95, 1.0];

/// Construct the platform clipboard. macOS gets `MacClipboard` with
/// full pasteboard parity (text, HTML, file paths, images). Other
/// platforms fall back to `InMemoryClipboard` — anyclaude is
/// macOS-targeted today and the legacy ui::run takes the same
/// approach.
fn make_clipboard() -> Box<dyn Clipboard> {
    #[cfg(target_os = "macos")]
    {
        Box::new(term_clipboard::MacClipboard::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(term_clipboard::InMemoryClipboard::default())
    }
}
