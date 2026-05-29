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
use term_clipboard::{
    get_image_filepaths_from_paths, pick_best_image, save_image_to_temp,
    should_insert_text_on_paste, Clipboard, ClipboardContent,
};
use term_core::{create_emulator, AnsiPalette, MouseMode, TerminalEmulator};
use term_gpu::{
    build_cursor_rect, encode_key, encode_paste, measure_cell_metrics, populate_panel,
    push_selection_rects, selection_to_text, shell_quote_path, CellMetrics, CellPoint, FontFamily,
    FontSystem, GlyphInstance, GpuRenderer, PanelRect, RectInstance, RenderLayer, ScrollState,
    SwashCache, TextShapeCache, GESTURE_END_TIMEOUT, MOMENTUM_FRAME_INTERVAL, NUM_PIXELS_PER_LINE,
};
use glam::Vec2;
use term_ui::{
    apply_overlay_alpha, build_root, free_subtree, measure, paint, place, place_centered,
    reconcile_root, Block, NodeId, PaintOutput, RetainedTree, SizeConstraint, Stack,
};
use uuid::Uuid;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{
    ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::backend::{AgentBackendState, BackendState};
use crate::config::{
    save_claude_settings, ClaudeSettingsManager, Config, SettingsFieldSnapshot,
};
use crate::metrics::ObservabilityHub;
use crate::ui::app_state::{ApplyCtx, AppState, Effect, Msg};
use crate::ui::chrome_labels;
use crate::ui::popup_anim::{popup_fade_alpha, step_popup_anim, PopupAnim};
use crate::ui::popup_view;
use crate::ui::backend_switch::{
    override_selection_to_backend_id, BackendPopupSection, BackendSwitchIntent, BackendSwitchState,
};
use crate::ui::gpu::pty::ChildPty;
use crate::ui::history::{HistoryEntry, HistoryIntent};
use crate::ui::input::{self, AppShortcut};
use crate::ui::settings::{SettingsDialogState, SettingsIntent};
use crate::ui::term_geometry;

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

/// Popup open/close fade duration (seconds).
const POPUP_FADE_SECS: f32 = 0.12;

use super::chrome::{
    CHROME_FONT_SIZE, CHROME_H_PAD, FOOTER_HEIGHT_LOGICAL, HEADER_HEIGHT_LOGICAL,
    SESSION_COPY_FLASH,
};

/// User event delivered to the winit loop. Drives redraws in response
/// to PTY output and scroll momentum without polling.
#[derive(Debug, Clone, Copy)]
pub(super) enum UserEvent {
    PtyBytesArrived,
    GestureEnded,
    MomentumTick,
    /// 1Hz heartbeat that keeps Uptime / Reqs / sub / team chrome
    /// fresh even when the PTY is silent.
    TickRedraw,
}


pub(super) struct GpuApp {
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

    /// Retained term_ui tree for the chrome overlay (header + footer). Built
    /// from `chrome_labels::chrome_view(&AppState)` each frame and reconciled
    /// against the prior view (bucket 2 — derived from AppState).
    chrome_tree: RetainedTree,
    chrome_root: Option<NodeId>,
    chrome_prev: Option<Stack>,
    chrome_scratch: PaintOutput,

    /// Retained term_ui tree for the popup overlay (history / settings / backend
    /// switch). Built from [`popup_view`] when a popup is open, centred with
    /// `place_centered`, and painted into the overlay on top of the chrome. A
    /// tree distinct from the chrome so the two reconcile independently. Every
    /// popup view is a `Block` (the popup box), so `popup_prev` is `Block`.
    /// (bucket 2 — derived from AppState.)
    popup_tree: RetainedTree,
    popup_root: Option<NodeId>,
    popup_prev: Option<Block>,
    popup_scratch: PaintOutput,
    /// Open/close fade epoch for the popup overlay (bucket 3-S). `None` when no
    /// fade is in flight; the alpha is derived from this + the frame clock.
    popup_anim: Option<PopupAnim>,

    // Lazily initialised in `resumed`: spawning the shell needs to know
    // the window's pixel size, which we don't have until then.
    pty: Option<ChildPty>,
    emulator: Option<Box<dyn TerminalEmulator>>,

    /// The single bucket-1 source of UI-decision truth — grid size, scroll +
    /// momentum, selection / input, session header, and the popup overlays.
    /// See [`AppState`]. (Resources, the emulator, and timer handles stay out
    /// here in the coordinator; bucket 3-S / 3-T.)
    state: AppState,

    momentum_abort: Option<AbortHandle>,
    gesture_end_abort: Option<AbortHandle>,
    /// 1Hz redraw heartbeat — see [`UserEvent::TickRedraw`]. Aborted
    /// implicitly when the proxy's send_event fails (window closed).
    periodic_tick_abort: Option<AbortHandle>,

    /// X range of the session click hot-zone (logical pixels) in the
    /// header. Updated every redraw so the click handler can hit-test
    /// without recomputing the layout. (Derived / materialized — bucket 2.)
    session_click_zone: Option<(f32, f32)>,

    clipboard: Box<dyn Clipboard>,

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
}


impl GpuApp {
    pub(super) fn new(
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
            chrome_tree: RetainedTree::new(),
            chrome_root: None,
            chrome_prev: None,
            chrome_scratch: PaintOutput::default(),
            popup_tree: RetainedTree::new(),
            popup_root: None,
            popup_prev: None,
            popup_scratch: PaintOutput::default(),
            popup_anim: None,
            palette: AnsiPalette::default_dark(),
            cell_metrics: None,
            pty: None,
            emulator: None,
            state: AppState::new(
                Uuid::new_v4().to_string(),
                Instant::now(),
                (INITIAL_GRID_COLS, INITIAL_GRID_ROWS),
            ),
            momentum_abort: None,
            gesture_end_abort: None,
            periodic_tick_abort: None,
            session_click_zone: None,
            clipboard: make_clipboard(),
            backend_state,
            subagent_backend,
            teammate_backend,
            observability,
            spawn_command,
            spawn_args,
            spawn_env,
            settings_manager,
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
        term_geometry::terminal_panel_rect(
            w_logical,
            h_logical,
            HEADER_HEIGHT_LOGICAL,
            FOOTER_HEIGHT_LOGICAL,
            CHROME_H_PAD,
        )
    }

    /// Compute the grid size (cols × rows) that fits inside the
    /// terminal panel rect at the current cell metrics. Both
    /// dimensions are clamped to at least 1 — a sub-cell terminal
    /// area is degenerate but should never panic.
    fn fit_grid(&mut self) -> (usize, usize) {
        let metrics = self.cell_metrics();
        let panel = self.terminal_panel_rect();
        term_geometry::fit_grid(
            panel,
            metrics.width_physical,
            metrics.height_physical,
            self.scale_factor,
        )
    }

    /// Resync emulator + PTY to the current window size. Called from
    /// `resumed` and on `Resized`/`ScaleFactorChanged`.
    fn resync_grid(&mut self) {
        let (cols, rows) = self.fit_grid();
        self.dispatch(Msg::GridResized { cols, rows });
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
        let was_at_bottom = self.state.scroll.offset_y <= SCROLL_BOTTOM_EPSILON;
        if let Some(emu) = self.emulator.as_mut() {
            for chunk in chunks {
                emu.process(&chunk);
            }
        }
        self.refresh_scroll_geometry();
        if was_at_bottom {
            self.state.scroll.offset_y = 0.0;
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
        self.state.scroll.total_size_px = snap.rows.len() as f32 * cell_h_logical;
        self.state.scroll.visible_px = visible_h_logical;
        let max = self.state.scroll.max_offset();
        if self.state.scroll.offset_y > max {
            self.state.scroll.offset_y = max;
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

    /// Translate a `Msg` to its state transition and perform the resulting
    /// effects: build the read-only `ApplyCtx`, call `AppState::apply`, then run
    /// each `Effect`. This is the single coordinator-side entry for the event
    /// loop — every winit / user event funnels through here. (Mouse press builds
    /// its own ctx carrying the emulator snapshot; see `on_mouse_press`.)
    fn dispatch(&mut self, msg: Msg) {
        let ctx = ApplyCtx { now: Instant::now(), snapshot: None };
        let effects = self.state.apply(msg, &ctx);
        self.perform_effects(effects);
    }

    /// Perform the side effects `apply` returned. The one place a state
    /// transition reaches a resource — timers, redraw, PTY / clipboard /
    /// renderer; the reducer stayed pure on `AppState` (bucket 3-S).
    fn perform_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::CancelMomentum => self.cancel_momentum(),
                Effect::CancelGestureEnd => self.cancel_gesture_end(),
                Effect::ScheduleMomentum => {
                    self.momentum_abort = Some(schedule_momentum_loop(
                        self.proxy.clone(),
                        MOMENTUM_FRAME_INTERVAL,
                    ));
                }
                Effect::ScheduleGestureEnd => {
                    self.gesture_end_abort = Some(schedule_once(
                        self.proxy.clone(),
                        GESTURE_END_TIMEOUT,
                        UserEvent::GestureEnded,
                    ));
                }
                Effect::Redraw => self.request_redraw(),
                Effect::ResizeEmulatorAndPty { cols, rows } => {
                    if let Some(emu) = self.emulator.as_mut() {
                        emu.resize(cols, rows);
                    }
                    if let Some(pty) = self.pty.as_ref() {
                        pty.resize(cols as u16, rows as u16);
                    }
                }
            }
        }
    }

    /// True when any popup is in its `Visible` state. Used to gate
    /// input routing and mouse-click priority.
    fn any_popup_visible(&self) -> bool {
        self.state.any_popup_visible()
    }

    /// Dispatch `Close` to every popup store. Called by Cmd+B / +H /
    /// +E before opening a new popup, by Esc, and by click-outside.
    fn close_all_popups(&mut self) {
        self.state.close_all_popups();
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Dismiss the open popup via Esc / click-outside. Settings gets the
    /// two-stage dirty-confirm: a first dismiss on unsaved changes arms a
    /// "discard?" prompt (the dialog stays open), a second discards — so it
    /// routes through `RequestClose`. The other popups carry no unsaved state
    /// and close immediately. (Enter-to-save still calls `close_all_popups`
    /// directly — saving needs no confirmation.)
    fn request_close_popups(&mut self) {
        if self.state.settings.is_visible() {
            self.state.settings.apply(SettingsIntent::RequestClose);
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
        } else {
            self.close_all_popups();
        }
    }

    /// Cmd+B handler — open or close the backend switch popup. Open
    /// dispatches the Open intent with the active backend pre-selected
    /// so pressing Enter is a no-op if the user is just inspecting.
    fn toggle_backend_switch_popup(&mut self) {
        if self.state.backend_switch.is_visible() {
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
        self.state.backend_switch.apply(BackendSwitchIntent::Open {
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
        if self.state.history.is_visible() {
            self.close_all_popups();
            return;
        }
        let entries = self.backend_state.get_switch_log();
        let history_entries: Vec<HistoryEntry> = entries
            .into_iter()
            .map(|e| HistoryEntry {
                timestamp: e.timestamp,
                from_backend: e.old_backend,
                to_backend: e.new_backend,
            })
            .collect();
        self.close_all_popups();
        self.state.history.apply(HistoryIntent::Load {
            entries: history_entries,
        });
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Cmd+E handler — open or close the settings popup. Field
    /// snapshots are loaded from `settings_manager`; Space toggles
    /// rows (marks state dirty), Enter applies and saves, Esc
    /// discards.
    fn toggle_settings_popup(&mut self) {
        if self.state.settings.is_visible() {
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
        self.state.settings.apply(SettingsIntent::Load { fields });
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Persist the settings popup's edits to disk. Reads the current
    /// popup state, applies each row to the manager, then calls
    /// `save_claude_settings`. Errors are logged but non-fatal.
    fn apply_settings_and_save(&mut self) {
        let fields = match &self.state.settings {
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
        if self.state.backend_switch.is_visible() {
            self.handle_backend_switch_key(event);
        } else if self.state.history.is_visible() {
            self.handle_history_key(event);
        } else if self.state.settings.is_visible() {
            self.handle_settings_key(event);
        }
    }

    fn handle_backend_switch_key(&mut self, event: &winit::event::KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        if let Some(intent) = input::backend_switch_nav(code) {
            self.state.backend_switch.apply(intent);
            self.request_redraw();
        } else if code == KeyCode::Enter {
            self.apply_backend_switch_selection();
            self.close_all_popups();
        }
    }

    /// Apply whichever action the active section maps to: the Active
    /// section calls `switch_backend`; the Subagent / Teammate sections
    /// write into their `AgentBackendState` (index 0 == Disabled
    /// → `None`, index N+1 == backend N). Errors are logged but
    /// non-fatal — the popup still closes.
    fn apply_backend_switch_selection(&mut self) {
        let (section, backend_sel, subagent_sel, teammate_sel) =
            match self.state.backend_switch {
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
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        if let Some(intent) = input::history_nav(code) {
            self.state.history.apply(intent);
            self.request_redraw();
        } else if code == KeyCode::Enter {
            self.close_all_popups();
        }
    }

    fn handle_settings_key(&mut self, event: &winit::event::KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        if let Some(intent) = input::settings_nav(code) {
            self.state.settings.apply(intent);
            self.request_redraw();
        } else if code == KeyCode::Enter {
            self.apply_settings_and_save();
            self.close_all_popups();
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
        let (cols, rows) = self.state.grid_size;
        self.emulator = Some(create_emulator(cols, rows, SCROLLBACK_LINES));
        self.state.scroll = ScrollState::default();
        self.state.scroll_velocity = None;
        self.cancel_momentum();
        self.cancel_gesture_end();
        self.state.selection = None;
        self.state.dragging_selection = false;
        self.state.last_click = None;

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
            .write(ClipboardContent::plain_text(self.state.session_id.clone()));
        self.state
            .mark_session_copied(Instant::now() + SESSION_COPY_FLASH);
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Copy the current selection to the system clipboard. Mirrors
    /// term_grid: `selection_to_text` against the current emulator
    /// snapshot → `ClipboardContent::plain_text`. Empty selections are
    /// skipped silently.
    fn copy_selection(&mut self) {
        let Some(sel) = self.state.selection else { return };
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
        let snap = emu.snapshot();
        let total_rows = snap.rows.len();
        let visible_rows = snap.visible_rows;
        let cols = snap.rows.first().map(|r| r.cells.len()).unwrap_or(0);
        term_geometry::cell_at(
            x,
            y,
            panel,
            metrics.width_physical,
            metrics.height_physical,
            self.scale_factor,
            self.state.scroll.offset_y,
            total_rows,
            visible_rows,
            cols,
        )
    }

    fn on_cursor_moved(&mut self, x: f32, y: f32) {
        self.state.set_cursor_pos(x, y);
        if !self.state.dragging_selection {
            return;
        }
        if let Some(point) = self.cell_at(x, y) {
            if self.state.drag_selection_to(point) {
                self.request_redraw();
            }
        }
    }

    fn on_mouse_press(&mut self) {
        let Some((x, y)) = self.state.cursor_pos else { return };
        // When a popup is open, a click anywhere dismisses it
        // (matching macOS modal-out behaviour) and is otherwise
        // swallowed — the click never starts a selection in the
        // terminal underneath.
        if self.any_popup_visible() {
            self.request_close_popups();
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
        let Some(snapshot) = self.emulator.as_ref().map(|e| e.snapshot()) else {
            return;
        };
        let count = self
            .state
            .next_click(point, Instant::now(), MULTI_CLICK_THRESHOLD_MS);
        self.state.begin_selection(point, count, &snapshot);
        self.request_redraw();
    }

    fn on_mouse_release(&mut self) {
        if self.state.end_selection_drag() {
            self.request_redraw();
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
        let scroll_offset_y = self.state.scroll.offset_y;
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
        if let Some(sel) = self.state.selection {
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

        // Chrome (header + footer) and any popup render in the OVERLAY layer,
        // which is drawn entirely AFTER the terminal base. So the bars' opaque
        // background covers any terminal glyph that scrolls into the bar band,
        // the bar text sits on top, and a popup sits on top of the bars.
        let mut overlay_shadows: Vec<term_gpu::ShadowInstance> = Vec::new();
        let mut overlay_rects: Vec<RectInstance> = Vec::new();
        let mut overlay_glyphs: Vec<GlyphInstance> = Vec::new();

        // The copied-flash is DERIVED from the deadline + frame clock (R12) —
        // no stored boolean, no expiry mutation.
        let now = Instant::now();
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
        let window_size = window.inner_size();
        let window_w_logical = window_size.width as f32 / sf;
        let window_h_logical = window_size.height as f32 / sf;
        // Chrome (header + footer) is a term_ui view now: build it from the
        // current AppState, reconcile against last frame, lay it out to the
        // full window, and paint it into the overlay layer.
        let header = chrome_labels::header_segments(
            &active_backend,
            subagent_label.as_deref(),
            teammate_label.as_deref(),
            total_reqs,
            self.state.uptime_secs(now),
            &self.state.session_id,
            self.state.session_copied(now),
        );
        let (footer_left, footer_right) = chrome_labels::footer_segments(env!("CARGO_PKG_VERSION"));
        let chrome = chrome_labels::chrome_view(
            &header,
            &footer_left,
            &footer_right,
            CHROME_FONT_SIZE,
            HEADER_HEIGHT_LOGICAL,
            FOOTER_HEIGHT_LOGICAL,
            CHROME_H_PAD,
        );
        let chrome_root = match self.chrome_root {
            Some(root) => {
                let prev = self
                    .chrome_prev
                    .take()
                    .expect("chrome_prev present once built");
                reconcile_root(&mut self.chrome_tree, root, &prev, &chrome);
                root
            }
            None => build_root(&mut self.chrome_tree, &chrome),
        };
        self.chrome_root = Some(chrome_root);
        self.chrome_prev = Some(chrome);
        measure(
            &mut self.chrome_tree,
            chrome_root,
            SizeConstraint::tight(Vec2::new(window_w_logical, window_h_logical)),
            &mut self.font_system,
            &mut self.ui_shape_cache,
            sf,
        );
        place(&mut self.chrome_tree, chrome_root, Vec2::ZERO);
        self.chrome_scratch.clear();
        paint(
            &self.chrome_tree,
            chrome_root,
            &mut self.chrome_scratch,
            renderer.atlas_mut(),
            &mut self.font_system,
            &mut self.swash_cache,
            &mut self.ui_shape_cache,
            sf,
        );
        overlay_rects.extend_from_slice(&self.chrome_scratch.rects);
        overlay_glyphs.extend_from_slice(&self.chrome_scratch.glyphs);
        // Re-derive the session-click hot-zone (x-range) from the laid-out
        // chrome tree: the "Session: …" run is tagged with a stable WidgetId,
        // so we resolve its node + bounds. `on_mouse_press` hit-tests against
        // it (the header-band y-gate handles the vertical extent).
        self.session_click_zone = self
            .chrome_tree
            .resolve_widget(chrome_labels::session_widget_id())
            .map(|nid| {
                let b = self.chrome_tree.node(nid).bounds;
                (b.origin.x, b.right())
            });
        // Popup overlay — all three popups render via the term_ui SECOND TREE.
        // The backend switch needs runtime data AppState doesn't carry (the
        // backend list + active/override ids), so it is built here via
        // popup_view::backend_view; history + settings come straight from
        // AppState via popup_view::popup_view. Whichever is open is reconciled
        // into the popup tree, measured with a min-width floor, centred with
        // place_centered, and painted into the overlay on top of the chrome (its
        // term_ui Block drop shadow flows through too). Popups are mutually
        // exclusive, so at most one is ever built.
        let popup: Option<Block> = if self.state.backend_switch.is_visible() {
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
            Some(popup_view::backend_view(
                &self.state.backend_switch,
                &items_and_ids,
                &active_backend,
                current_subagent.as_deref(),
                current_teammate.as_deref(),
            ))
        } else {
            popup_view::popup_view(&self.state)
        };
        // Open/close fade (R12): advance the epoch on a visibility EDGE, then
        // derive this frame's alpha from the frame clock (pure helpers in
        // `popup_anim`). `popup_animating` keeps the redraw loop alive
        // (self-requested below) until the fade completes.
        let visible = popup.is_some();
        self.popup_anim = step_popup_anim(self.popup_anim, visible, now);
        let (popup_alpha, popup_animating) =
            popup_fade_alpha(self.popup_anim, now, POPUP_FADE_SECS);

        // Pick the root to paint this frame: the live popup (reconciled into the
        // tree), or — during a fade-OUT, when the store is already Hidden — the
        // retained tree kept alive at the decreasing alpha until the fade ends.
        let popup_root_to_paint: Option<NodeId> = if let Some(view) = popup {
            let root = match self.popup_root {
                Some(root) => {
                    let prev = self
                        .popup_prev
                        .take()
                        .expect("popup_prev present once built");
                    reconcile_root(&mut self.popup_tree, root, &prev, &view);
                    root
                }
                None => build_root(&mut self.popup_tree, &view),
            };
            self.popup_root = Some(root);
            self.popup_prev = Some(view);
            Some(root)
        } else if popup_animating {
            // Fade-OUT in flight: keep painting the frozen retained tree.
            self.popup_root
        } else {
            // No popup + no fade — release the retained tree and reset the epoch.
            if let Some(root) = self.popup_root.take() {
                free_subtree(&mut self.popup_tree, root);
            }
            self.popup_prev = None;
            self.popup_anim = None;
            None
        };
        if let Some(root) = popup_root_to_paint {
            measure(
                &mut self.popup_tree,
                root,
                SizeConstraint::new(
                    Vec2::new(popup_view::POPUP_MIN_WIDTH, 0.0),
                    Vec2::new(window_w_logical, window_h_logical),
                ),
                &mut self.font_system,
                &mut self.ui_shape_cache,
                sf,
            );
            place_centered(
                &mut self.popup_tree,
                root,
                Vec2::new(window_w_logical, window_h_logical),
            );
            self.popup_scratch.clear();
            paint(
                &self.popup_tree,
                root,
                &mut self.popup_scratch,
                renderer.atlas_mut(),
                &mut self.font_system,
                &mut self.swash_cache,
                &mut self.ui_shape_cache,
                sf,
            );
            // Bake the fade alpha into the popup's instances only (the chrome
            // beneath, already merged, keeps full opacity).
            if popup_alpha < 1.0 {
                apply_overlay_alpha(&mut self.popup_scratch, popup_alpha);
            }
            overlay_shadows.extend_from_slice(&self.popup_scratch.shadows);
            overlay_rects.extend_from_slice(&self.popup_scratch.rects);
            overlay_glyphs.extend_from_slice(&self.popup_scratch.glyphs);
        }
        // The overlay always carries the chrome bars (and a popup when one is
        // open), so it is never empty.
        window.pre_present_notify();
        renderer.render(
            RenderLayer::rects_and_glyphs(&rects, &glyphs),
            Some(RenderLayer {
                shadows: &overlay_shadows,
                rects: &overlay_rects,
                glyphs: &overlay_glyphs,
            }),
            0.0,
        );
        self.shape_cache.end_frame();
        self.ui_shape_cache.end_frame();
        // Drive the popup fade to completion: while a transition is in flight,
        // request the next frame (the event-driven redraws alone wouldn't tick).
        if popup_animating {
            window.request_redraw();
        }
    }
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
        self.state.grid_size = (cols, rows);
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
                self.dispatch(Msg::GestureEnd);
            }
            UserEvent::MomentumTick => {
                self.refresh_scroll_geometry();
                self.dispatch(Msg::MomentumTick);
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
                // resync_grid dispatches Msg::GridResized → apply updates the
                // grid + asks for the emulator/PTY resize + redraw as effects.
                self.resync_grid();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                if let Some(r) = self.renderer.as_mut() {
                    r.set_scale_factor(self.scale_factor);
                }
                // Cell metrics depend on scale_factor; invalidate, then resync
                // the grid to the new physical cell size (through the loop).
                self.cell_metrics = None;
                self.resync_grid();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.dispatch(Msg::ModifiersChanged(mods.state()));
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let (precise, dy) = match delta {
                    MouseScrollDelta::PixelDelta(p) => (true, p.y as f32),
                    MouseScrollDelta::LineDelta(_, v) => (false, v * NUM_PIXELS_PER_LINE),
                };
                self.refresh_scroll_geometry();
                self.dispatch(Msg::Wheel { dy, phase, precise });
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
                        self.request_close_popups();
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
                if self.state.modifiers.super_key() {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        if let Some(shortcut) = input::app_shortcut(code, self.state.modifiers) {
                            match shortcut {
                                AppShortcut::CopySelection => self.copy_selection(),
                                AppShortcut::Paste => self.paste_into_pty(),
                                AppShortcut::ToggleBackendPopup => {
                                    self.toggle_backend_switch_popup()
                                }
                                AppShortcut::ToggleHistoryPopup => self.toggle_history_popup(),
                                AppShortcut::ToggleSettingsPopup => self.toggle_settings_popup(),
                                AppShortcut::RestartPty => self.restart_pty(),
                                AppShortcut::DumpDiagnostic => {
                                    let snap = self.emulator.as_ref().map(|e| e.snapshot());
                                    super::diagnostic::dump_snapshot(
                                        self.state.grid_size,
                                        self.state.scroll.offset_y,
                                        self.state.scroll.max_offset(),
                                        snap.as_ref(),
                                    );
                                }
                                AppShortcut::Quit => event_loop.exit(),
                            }
                        }
                    }
                    return;
                }
                // Ctrl combos belong to the shell (Ctrl+C / Ctrl+D /
                // ...) and pass straight through encode_key.
                let Some(bytes) = encode_key(&event.logical_key, self.state.modifiers) else {
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
