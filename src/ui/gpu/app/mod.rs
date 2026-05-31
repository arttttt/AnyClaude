//! Winit `ApplicationHandler` for the GPU UI — the `GpuApp` coordinator.
//!
//! `GpuApp` owns the window / renderer / scale factor, the bucket-1 [`AppState`]
//! truth, and a handful of collaborators that carry the rest of the world out of
//! the coordinator: [`TextResources`] (rasterization), [`OverlayRenderer`] (the
//! chrome + popup term_ui trees and their paint pipeline), [`Session`] (PTY +
//! emulator + spawn params), [`Timers`] (momentum / gesture-end / heartbeat),
//! and [`Backends`] (proxy + config handles).
//!
//! The coordinator's own behaviour is split across responsibility submodules,
//! each an `impl super::GpuApp` block that sees these private fields directly:
//!   - [`events`]    — the `Msg` → `apply` → `Effect` loop + `ApplicationHandler`
//!   - [`render`]    — the per-frame paint (`redraw`)
//!   - [`geometry`]  — cell metrics, panel/grid fit, scroll bounds, mouse hit-test
//!   - [`popups`]    — the three popup toggles + their apply/save handlers
//!   - [`clipboard`] — copy session id / copy selection / paste
//!   - [`session_ops`] — drain the PTY / restart the session

use std::sync::Arc;
use std::time::Instant;
use term_clipboard::Clipboard;
use term_gpu::GpuRenderer;
use term_ui::Bounds;
use uuid::Uuid;
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

use crate::backend::{AgentBackendState, BackendState};
use crate::config::ClaudeSettingsManager;
use crate::metrics::ObservabilityHub;
use crate::ui::app_state::AppState;

use super::backends::Backends;
use super::overlay::OverlayRenderer;
use super::session::Session;
use super::text::TextResources;
use super::timers::Timers;

mod clipboard;
mod events;
mod geometry;
mod popups;
mod render;
mod session_ops;

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

    /// Terminal + chrome text-rasterization resources (font system, swash +
    /// shape caches, palette, cached cell metrics). See [`TextResources`].
    text: TextResources,

    /// The overlay renderer: the chrome + popup retained term_ui trees and the
    /// popup fade epoch, plus the term_ui pipeline that paints them on top of
    /// the terminal grid. See [`OverlayRenderer`].
    overlay: OverlayRenderer,

    /// Terminal session — the PTY child, the VT emulator, and the spawn params.
    /// Lazily populated in `resumed`. See [`Session`].
    session: Session,

    /// The single bucket-1 source of UI-decision truth — grid size, scroll +
    /// momentum, selection / input, session header, and the popup overlays.
    /// See [`AppState`]. (Resources, the emulator, and timer handles stay out
    /// here in the coordinator; bucket 3-S / 3-T.)
    state: AppState,

    /// Background timers (momentum decay, the gesture-end silence fallback, the
    /// 1 Hz chrome heartbeat) — see [`Timers`].
    timers: Timers,

    /// X range of the session click hot-zone (logical pixels) in the
    /// header. Updated every redraw so the click handler can hit-test
    /// without recomputing the layout. (Derived / materialized — bucket 2.)
    session_click_zone: Option<(f32, f32)>,

    /// Right teammates overlay hit-zones (logical px), materialized each redraw
    /// so the mouse handler can hit-test without re-laying-out the tree. The
    /// whole overlay rect (clicks inside are swallowed from the terminal) and
    /// the toggle/indicator button bounds (click → collapse/expand). `None` when
    /// the overlay isn't rendered. (Derived — bucket 2.)
    panel_overlay_rect: Option<Bounds>,
    panel_toggle_zone: Option<Bounds>,

    clipboard: Box<dyn Clipboard>,

    /// Proxy + config handles — backend state, subagent / teammate overrides,
    /// observability, settings manager. See [`Backends`].
    backends: Backends,
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
            text: TextResources::new(),
            overlay: OverlayRenderer::new(),
            session: Session::new(spawn_command, spawn_args, spawn_env),
            state: AppState::new(
                Uuid::new_v4().to_string(),
                Instant::now(),
                (INITIAL_GRID_COLS, INITIAL_GRID_ROWS),
            ),
            timers: Timers::new(),
            session_click_zone: None,
            panel_overlay_rect: None,
            panel_toggle_zone: None,
            clipboard: make_clipboard(),
            backends: Backends {
                backend_state,
                subagent_backend,
                teammate_backend,
                observability,
                settings_manager,
            },
        }
    }

    fn request_redraw(&self) {
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }
}

/// Construct the platform clipboard. macOS gets `MacClipboard` with
/// full pasteboard parity (text, HTML, file paths, images). Other
/// platforms fall back to `InMemoryClipboard` — anyclaude is
/// macOS-targeted today.
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
