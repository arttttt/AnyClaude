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
use std::time::{Duration, Instant};
use term_clipboard::Clipboard;
use term_gpu::GpuRenderer;
use term_ui::{Animation, Bounds, Interpolator, Spring};
use uuid::Uuid;
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

use crate::backend::{AgentBackendState, BackendState};
use crate::config::ClaudeSettingsManager;
use crate::metrics::ObservabilityHub;
use crate::ui::app_state::AppState;
use crate::ui::child_session::ChildSessionManager;
use crate::ui::gpu::panes::Panes;

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

/// Panel overlay collapse/expand width-slide duration (seconds).
const PANEL_ANIM_SECS: f32 = 0.14;

/// Initial grid a teammate pane spawns at, before the first render resizes it to
/// its page rect. Small — it only matters for the first frame.
const INITIAL_PANE_GRID: (usize, usize) = (40, 12);

/// Inner padding (logical px) of a teammate pane's grid inside its page, so text
/// never touches the frame. The LEFT inset is a bit wider so the grid clears the
/// collapse pill centred on the divider (and never draws over it).
const PANE_PAD: f32 = 4.0;
const PANE_PAD_LEFT: f32 = 12.0;

/// Pager page-settle spring constants (page units). `DAMPING ≈ 2·√STIFFNESS` is
/// critical — snappy, no overshoot.
const PAGE_SPRING_STIFFNESS: f32 = 700.0;
const PAGE_SPRING_DAMPING: f32 = 53.0;
/// Max `|dx|` (logical px) of a `Started` event that counts as a fresh
/// finger-down. A real touch begins from REST (its first event is tiny — a few
/// px); macOS momentum BEGINS at the release velocity (winit reports its start as
/// a large-`dx` `Started`). So a small-velocity `Started` is a genuine new swipe
/// — even one interrupting the previous flick's momentum — and a large one is
/// just inertia, ignored. (Logged touches start ~2-8 px, momentum ~54-94 px.)
const PAGE_SWIPE_START_VELOCITY: f32 = 30.0;
/// Release speed (pages/sec) above which a swipe is a FLING — it advances one
/// page in its direction even if dragged under halfway (Flutter's ±0.5 nudge);
/// below it the page snaps to whichever side it was dragged past.
const PAGE_SWIPE_FLING_VELOCITY: f32 = 1.5;

/// State of the pager's horizontal two-finger swipe (a trackpad gesture, NOT a
/// mouse-button drag — that would fight text selection inside a page). While
/// `active`, the page tracks the finger from `start_scroll` by `accum_px` of
/// travel; `velocity` (pages/sec) drives the release snap.
#[derive(Debug, Clone, Copy)]
struct PageSwipe {
    active: bool,
    start_scroll: f32,
    accum_px: f32,
    velocity: f32,
    last_t: Instant,
}

/// User event delivered to the winit loop. Drives redraws in response
/// to PTY output and scroll momentum without polling. NOT `Copy`/`Clone` — the
/// `ControlPlane` variant carries a one-shot reply channel that must move.
#[derive(Debug)]
pub(super) enum UserEvent {
    PtyBytesArrived,
    /// A teammate pane's PTY reader queued new bytes (the per-pane analogue of
    /// `PtyBytesArrived`); the coordinator drains that pane's surface.
    PtyBytes(crate::ui::child_session::PaneId),
    GestureEnded,
    MomentumTick,
    /// 1Hz heartbeat that keeps Uptime / Reqs / sub / team chrome
    /// fresh even when the PTY is silent.
    TickRedraw,
    /// A teammate lifecycle request from the proxy's tmux control plane
    /// (tokio side). The coordinator applies it and answers its reply channel
    /// with the resulting `PaneId` (the tmux `%N`).
    ControlPlane(crate::ui::control_plane::ControlRequest),
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

    /// Registry + lifecycle for teammate child sessions (bucket 3 — identity).
    /// Reacts to `ChildSessionEvent`s by orchestrating `state.right`. See
    /// [`ChildSessionManager`].
    child_sessions: ChildSessionManager,

    /// Teammate pane resources (bucket 3-T): the live `TerminalSurface`s keyed by
    /// `PaneId`. Spawned on `Register`, dropped on `Unregister`, drained on
    /// `PtyBytes`. See [`Panes`].
    panes: Panes,

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

    /// Right overlay width tween (bucket 3-S): the collapse/expand slide AND the
    /// live drag width, as one `Animation`. The rendered width is `value(now)` —
    /// derived each frame, never stored (R12). `retarget` drives the button
    /// slide; `snap` tracks a hand-drag.
    panel_width: Animation<f32>,

    /// Right overlay pager position (bucket 3-S): the continuous page index, in
    /// page units, as a [`Spring`]. Its target chases the focused panel's index
    /// each frame, so paging (hotkey / click / two-finger swipe) slides.
    page_scroll: Spring,
    /// Horizontal two-finger swipe accumulator (bucket 2) that pages the overlay.
    page_swipe: PageSwipe,

    /// The mouse cursor icon currently set on the window — cached so a hover move
    /// only calls `set_cursor` on a CHANGE (a resize cursor over a panel edge, a
    /// pointer over the toggle pill, else the default).
    current_cursor: winit::window::CursorIcon,

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
        let state = AppState::new(
            Uuid::new_v4().to_string(),
            Instant::now(),
            (INITIAL_GRID_COLS, INITIAL_GRID_ROWS),
        );
        // The right overlay starts collapsed at its bare strip width; the first
        // redraw retargets it to the live state (a no-op while collapsed).
        let panel_width = Animation::settled(
            state.right.policy().collapsed_width,
            Instant::now(),
            Duration::from_secs_f32(PANEL_ANIM_SECS),
            Interpolator::EaseInOut,
        );
        // The pager starts on the first page; its spring target chases the
        // focused index (paging) or is driven by a swipe.
        let page_scroll = Spring::new(
            0.0,
            PAGE_SPRING_STIFFNESS,
            PAGE_SPRING_DAMPING,
            Instant::now(),
        );
        Self {
            proxy,
            window: None,
            renderer: None,
            scale_factor: 1.0,
            text: TextResources::new(),
            overlay: OverlayRenderer::new(Duration::from_secs_f32(POPUP_FADE_SECS)),
            session: Session::new(spawn_command, spawn_args, spawn_env),
            child_sessions: ChildSessionManager::new(),
            panes: Panes::new(SCROLLBACK_LINES),
            state,
            timers: Timers::new(),
            session_click_zone: None,
            panel_overlay_rect: None,
            panel_toggle_zone: None,
            panel_width,
            page_scroll,
            page_swipe: PageSwipe {
                active: false,
                start_scroll: 0.0,
                accum_px: 0.0,
                velocity: 0.0,
                last_t: Instant::now(),
            },
            current_cursor: winit::window::CursorIcon::Default,
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
