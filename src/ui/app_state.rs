//! `AppState` — the single bucket-1 source of UI-decision truth for the GPU app
//! (design R2/R3). Plain data: no `Rc`/`RefCell`, no GPU handles, no emulator.
//!
//! Phase E folds the GPU UI's scattered `GpuApp` decision fields into this one
//! struct, so there is exactly ONE place that holds "what the UI has decided"
//! (the split-brain the MVI drop was about). Resources (renderer, fonts, PTY,
//! clipboard, backend handles) stay outside in the coordinator (bucket 3-S);
//! the terminal emulator content stays in its own bucket (3-T). Today `GpuApp`
//! still owns those and drives transitions imperatively; the term_ui
//! coordinator that replaces it will mutate this through `apply`.
//!
//! Derived facts (the copied-flash boolean, uptime) are COMPUTED from epochs +
//! a frame clock here, never stored as resolved values (R12).

use std::time::Instant;

use glam::Vec2;
use term_core::RenderSnapshot;
use term_gpu::{
    decay_velocity, encode_key, expand_line, expand_word, CellPoint, ScrollState, ScrollVelocity,
    Selection, MOMENTUM_MIN_VELOCITY, MOMENTUM_THRESHOLD,
};
use winit::event::TouchPhase;
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

use crate::ui::backend_switch::{BackendSwitchIntent, BackendSwitchState};
use crate::ui::history::{HistoryDialogState, HistoryIntent};
use crate::ui::input::{self, AppShortcut};
use crate::ui::panel_manager::{PanelManager, Policy};
use crate::ui::settings::{SettingsDialogState, SettingsIntent};
use crate::ui::term_geometry::LastClick;

/// The authoritative UI-decision state. One writer per fact.
pub struct AppState {
    // Terminal grid sizing (cols × rows), recomputed on resize.
    pub grid_size: (usize, usize),

    // Retained scroll position + in-flight momentum (R11).
    pub scroll: ScrollState,
    pub scroll_velocity: Option<ScrollVelocity>,

    // Input + selection.
    pub modifiers: ModifiersState,
    /// Last mouse position in logical pixels (top-left origin).
    pub cursor_pos: Option<(f32, f32)>,
    pub dragging_selection: bool,
    pub selection: Option<Selection>,
    pub last_click: Option<LastClick>,
    /// Whether the left button is held while a mouse-tracking app is active —
    /// gates drag (motion-with-button) reporting (§6). Distinct from
    /// `dragging_selection`, which is suppressed under tracking.
    pub mouse_left_held: bool,
    /// The last cell a motion report was emitted for, so drag / any-event
    /// motion is reported once per cell crossed, not once per pixel (§6).
    pub mouse_motion_cell: Option<(u16, u16)>,

    // Session header state.
    pub session_id: String,
    /// Process-start epoch; the header's "Uptime" is derived from it (R12).
    pub start_time: Instant,
    /// While `Some(deadline)`, the header flashes "Session ID copied!" until
    /// `deadline`. The displayed boolean is DERIVED (`session_copied`), not
    /// stored (R12).
    pub session_copied_until: Option<Instant>,

    // Popup overlays (each a plain `apply()` state machine).
    pub backend_switch: BackendSwitchState,
    pub history: HistoryDialogState,
    pub settings: SettingsDialogState,

    // Multi-instance panel managers — one reusable type, two instances (R10:
    // nested UI-decision truth inside the one AppState). `left` is the sessions
    // sidebar (lands in a later milestone), `right` is the teammates overlay.
    // Both start empty + collapsed, so they're inert until populated/shown.
    pub left: PanelManager,
    pub right: PanelManager,
}

/// A side effect [`AppState::apply`] asks the coordinator to perform. `apply` is
/// pure on `AppState` (+ a read-only [`ApplyCtx`]); everything that touches a
/// resource — timers, redraw, PTY / clipboard / renderer writes — comes back as
/// one of these for `GpuApp::perform_effects` to run (bucket 3-S). Variants are
/// added as each event category is migrated onto the loop (E.8).
///
/// This is a command bus, and its surface grows with features: `apply` (the
/// emitter) and `perform_effects` (the handler) are coupled by an exhaustive
/// `match`. That coupling is intentional — a new variant can't be added without
/// the compiler forcing a handler — so the linear growth is the accepted cost of
/// keeping the reducer pure rather than a sign of drift.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Redraw,
    /// Start the momentum-tick loop (coordinator owns the abort handle).
    ScheduleMomentum,
    /// Abort the in-flight momentum loop.
    CancelMomentum,
    /// Arm the silence-timeout fallback that fires `GestureEnded` (non-precise
    /// wheels that never emit `TouchPhase::Ended`).
    ScheduleGestureEnd,
    /// Abort the pending gesture-end fallback.
    CancelGestureEnd,
    /// Resize the emulator + PTY to a new `cols × rows` grid after a window /
    /// scale change (the `grid_size` transition itself happens in `apply`).
    ResizeEmulatorAndPty { cols: usize, rows: usize },
    /// Write encoded key/paste bytes to the PTY (a terminal-focused keypress).
    WriteToPty(Vec<u8>),
    /// Open-or-close a popup (reads resources — backend list / settings registry
    /// / switch log — so the coordinator performs it via its toggle methods).
    ToggleBackendPopup,
    ToggleHistoryPopup,
    ToggleSettingsPopup,
    /// Close every open popup (state-only; the redraw is a separate `Redraw`).
    ClosePopups,
    /// Apply the backend-switch popup's current selection (writes backend state).
    ApplyBackendSelection,
    /// Persist the settings popup's edits to disk.
    SaveSettings,
    /// Copy the current selection to the clipboard.
    CopySelection,
    /// Copy the session id to the clipboard + arm the header "copied!" flash.
    CopySessionId,
    /// Read the clipboard and paste into the PTY.
    Paste,
    /// Tear down + respawn the Claude session (Cmd+R).
    RestartPty,
    /// Dump a diagnostic snapshot to stderr (Cmd+Shift+D).
    DumpDiagnostic,
    /// Debug-only: seed placeholder teammates + toggle the right overlay (the
    /// Milestone-1 panels experiment trigger; coordinator-side, debug builds).
    DebugTogglePanels,
    /// Exit the app (Cmd+Q / window close). Performed by the coordinator, which
    /// owns the `ActiveEventLoop` — surfaced as `perform_effects`' return.
    Quit,
    /// Drain pending PTY bytes into the emulator, redrawing if any arrived.
    Drain,
}

/// An input event translated to a pure message. The coordinator does any
/// read-only resource resolution needed to build a `Msg` (resolving a cell,
/// reading the backend list, …) so the message carries plain data; [`AppState::
/// apply`] then performs the state transition and returns [`Effect`]s. Variants
/// land as each event category is migrated (E.8).
pub enum Msg {
    /// A wheel / trackpad scroll delta. The coordinator refreshes scroll bounds
    /// (`scroll.total_size_px` / `visible_px`) before dispatching. When a
    /// mouse-reporting app is active, `mouse_report` carries the encoded wheel
    /// button to forward instead of scrolling our scrollback.
    Wheel { dy: f32, phase: TouchPhase, precise: bool, mouse_report: Option<Vec<u8>> },
    /// A scroll gesture ended (or its silence-timeout fallback fired).
    GestureEnd,
    /// One momentum-decay frame. Coordinator refreshes scroll bounds first.
    MomentumTick,
    /// The keyboard modifier state changed.
    ModifiersChanged(ModifiersState),
    /// The window / scale changed and the coordinator recomputed the terminal
    /// grid to `cols × rows` (computing it needs cell metrics — a resource — so
    /// the coordinator does that and hands the result in).
    GridResized { cols: usize, rows: usize },
    /// A key was pressed. `apply` runs the full routing — popup nav / app
    /// shortcut / terminal key — reading `modifiers` + popup visibility from
    /// `AppState`, and emits effects for everything that touches a resource.
    /// `logical_unmod` is the key without modifiers (the un-composed base char,
    /// for the Meta form); `app_cursor` is the emulator's DECCKM state, both
    /// pre-resolved by the coordinator. (§ key encoding)
    Key {
        logical: Key,
        logical_unmod: Key,
        physical: PhysicalKey,
        app_cursor: bool,
    },
    /// The cursor moved to `(x, y)` logical px. `point` is the cell under it,
    /// pre-resolved by the coordinator (when a selection drag is in flight OR a
    /// mouse-reporting app wants motion). `motion_report` is the encoded drag /
    /// move to forward when an app is tracking motion (§6), already deduped per
    /// cell by the coordinator; `None` otherwise.
    CursorMoved {
        x: f32,
        y: f32,
        point: Option<CellPoint>,
        motion_report: Option<Vec<u8>>,
    },
    /// A left mouse press. The coordinator pre-resolves the header / session-zone
    /// / mouse-reporting gates + the cell under the cursor; `apply` decides
    /// dismiss-popup vs copy-session vs begin-selection.
    MousePress {
        in_header: bool,
        in_session_zone: bool,
        point: Option<CellPoint>,
        /// When a mouse-reporting app is active, the encoded press to forward to
        /// the PTY (the selection is then suppressed); `None` otherwise.
        mouse_report: Option<Vec<u8>>,
    },
    /// A left mouse release (ends a drag-selection, or forwards a release to a
    /// mouse-reporting app via `mouse_report`).
    MouseRelease { mouse_report: Option<Vec<u8>> },
    /// A pre-encoded mouse report to forward verbatim to the PTY — the
    /// middle / right buttons (which have no local action), so the coordinator
    /// only builds this when a tracking mode is active. (§6)
    MouseReport(Vec<u8>),
    /// 1 Hz heartbeat — refresh the chrome (uptime / reqs) even when idle.
    Tick,
    /// The window close button was clicked (Cmd+Q routes via the Quit shortcut).
    Close,
    /// The PTY signalled that new output is ready to drain.
    PtyBytes,
}

/// Read-only context the coordinator supplies to [`AppState::apply`]: the frame
/// clock, plus (for selection) the current emulator snapshot. Resource WRITES
/// never happen through this — they come back as [`Effect`]s.
///
/// `snapshot` is `None` on the common path (`GpuApp::dispatch`); only the
/// mouse-press path (`on_mouse_press`) builds a ctx that carries it, because
/// only word/line selection-expansion needs the grid content. That's a
/// deliberate two-entry seam into `apply`: threading the snapshot through every
/// event would clone it per keystroke / tick for nothing. Cheaper than the
/// uniform alternative, but a seam worth keeping an eye on.
pub struct ApplyCtx<'a> {
    pub now: Instant,
    /// The emulator's current content, for selection word / line expansion.
    /// `None` when no emulator is live or the transition doesn't need it.
    pub snapshot: Option<&'a RenderSnapshot>,
    /// Max ms between presses at the same cell to count as a multi-click
    /// (coordinator UX tuning, passed in so `AppState` stays config-free).
    pub multi_click_threshold_ms: u128,
}

impl AppState {
    /// The single state-TRANSITION point: route a [`Msg`] to its transition and
    /// return the [`Effect`]s the coordinator must perform. Pure on `AppState` +
    /// the read-only `ctx`; every side effect comes back as data, so the reducer
    /// is unit-testable without a window.
    ///
    /// It is deliberately NOT the single *decision* point — calling it that would
    /// oversell it. The reduce/resolve boundary is drawn at "does it need to read
    /// a resource": the coordinator pre-resolves resource-backed facts before
    /// building a `Msg` (the cell under the cursor, the backend list, the
    /// emulator's DECCKM + mouse-protocol state), and folds the genuinely
    /// decision-shaped parts into PURE, separately-tested helpers it calls during
    /// that resolution — `term_gpu::encode_key` / `encode_mouse_report` /
    /// `encode_motion_report` (the latter owns the motion per-cell dedup + the
    /// tracking-level gate). So `apply` frequently receives half-resolved input
    /// (e.g. `Msg::Wheel { mouse_report: Some(bytes) }`) and its arm is trivial
    /// *because* the coordinator already did the resource-reading half. The logic
    /// stays pure and tested — it's just split between here and those helpers
    /// along the resource-read line, not centralized. The alternative (a fat
    /// `ApplyCtx` carrying a whole world snapshot so every decision lives here)
    /// was rejected: it would clone the snapshot on every event and bloat the
    /// ctx. Thin ctx + pure helpers is the trade.
    pub fn apply(&mut self, msg: Msg, ctx: &ApplyCtx) -> Vec<Effect> {
        match msg {
            Msg::Wheel { dy, phase, precise, mouse_report } => {
                // A mouse-reporting app owns the wheel — forward it, don't scroll.
                if let Some(bytes) = mouse_report {
                    return vec![Effect::WriteToPty(bytes)];
                }
                self.on_wheel(dy, phase, precise, ctx.now)
            }
            Msg::GestureEnd => self.on_gesture_end(ctx.now),
            Msg::MomentumTick => self.on_momentum_tick(ctx.now),
            Msg::ModifiersChanged(m) => {
                self.modifiers = m;
                Vec::new()
            }
            Msg::GridResized { cols, rows } => {
                let mut fx = Vec::new();
                if self.grid_size != (cols, rows) {
                    self.grid_size = (cols, rows);
                    fx.push(Effect::ResizeEmulatorAndPty { cols, rows });
                }
                fx.push(Effect::Redraw);
                fx
            }
            Msg::Key { logical, logical_unmod, physical, app_cursor } => {
                self.on_key(logical, logical_unmod, physical, app_cursor)
            }
            Msg::CursorMoved { x, y, point, motion_report } => {
                self.set_cursor_pos(x, y);
                // A mouse-reporting app owns motion — forward the drag / move
                // report (the coordinator already deduped it per cell) and skip
                // local selection.
                if let Some(bytes) = motion_report {
                    if let Some(p) = point {
                        self.mouse_motion_cell = Some((p.col as u16, p.row as u16));
                    }
                    return vec![Effect::WriteToPty(bytes)];
                }
                if self.dragging_selection {
                    if let Some(p) = point {
                        if self.drag_selection_to(p) {
                            return vec![Effect::Redraw];
                        }
                    }
                }
                Vec::new()
            }
            Msg::MousePress { in_header, in_session_zone, point, mouse_report } => {
                // A click while a popup is open dismisses it (and is swallowed).
                if self.any_popup_visible() {
                    return self.close_popups_or_request();
                }
                // Header click: copy the session id when it lands on the run.
                if in_header {
                    return if in_session_zone { vec![Effect::CopySessionId] } else { Vec::new() };
                }
                // A mouse-reporting app owns the click — forward, don't select.
                // Mark the left button held + seed the motion-dedup cell so a
                // following drag reports per cell crossed (§6).
                if let Some(bytes) = mouse_report {
                    self.mouse_left_held = true;
                    self.mouse_motion_cell = point.map(|p| (p.col as u16, p.row as u16));
                    return vec![Effect::WriteToPty(bytes)];
                }
                let (Some(p), Some(snap)) = (point, ctx.snapshot) else {
                    return Vec::new();
                };
                let count = self.next_click(p, ctx.now, ctx.multi_click_threshold_ms);
                self.begin_selection(p, count, snap);
                vec![Effect::Redraw]
            }
            Msg::MouseRelease { mouse_report } => {
                self.mouse_left_held = false;
                if let Some(bytes) = mouse_report {
                    return vec![Effect::WriteToPty(bytes)];
                }
                if self.end_selection_drag() {
                    vec![Effect::Redraw]
                } else {
                    Vec::new()
                }
            }
            Msg::MouseReport(bytes) => vec![Effect::WriteToPty(bytes)],
            Msg::Tick => vec![Effect::Redraw],
            Msg::Close => vec![Effect::Quit],
            Msg::PtyBytes => vec![Effect::Drain],
        }
    }

    /// Dismiss the open popup (Esc / click-outside): settings gets the two-stage
    /// dirty-confirm (`RequestClose`); the others carry no unsaved state and
    /// close immediately. Always asks for a redraw.
    fn close_popups_or_request(&mut self) -> Vec<Effect> {
        if self.settings.is_visible() {
            self.settings.apply(SettingsIntent::RequestClose);
        } else {
            self.close_all_popups();
        }
        vec![Effect::Redraw]
    }

    /// Route a key press. Popups own input while open; the clipboard (Cmd+C/V)
    /// and app features (a single Ctrl chord) are app shortcuts resolved before
    /// terminal encoding; an unbound Cmd combo is swallowed (never leaked to the
    /// PTY); everything else is a terminal key encoded via `encode_key`.
    fn on_key(
        &mut self,
        logical: Key,
        logical_unmod: Key,
        physical: PhysicalKey,
        app_cursor: bool,
    ) -> Vec<Effect> {
        if self.any_popup_visible() {
            return self.on_popup_key(physical);
        }
        if let PhysicalKey::Code(code) = physical {
            if let Some(shortcut) = input::app_shortcut(code, self.modifiers) {
                return vec![match shortcut {
                    AppShortcut::CopySelection => Effect::CopySelection,
                    AppShortcut::Paste => Effect::Paste,
                    AppShortcut::ToggleBackendPopup => Effect::ToggleBackendPopup,
                    AppShortcut::ToggleHistoryPopup => Effect::ToggleHistoryPopup,
                    AppShortcut::ToggleSettingsPopup => Effect::ToggleSettingsPopup,
                    AppShortcut::RestartPty => Effect::RestartPty,
                    AppShortcut::DumpDiagnostic => Effect::DumpDiagnostic,
                    AppShortcut::DebugTogglePanels => Effect::DebugTogglePanels,
                    AppShortcut::Quit => Effect::Quit,
                }];
            }
        }
        // A Cmd combo with no bound shortcut is swallowed — Cmd+key has no
        // terminal byte and must not leak to the PTY.
        if self.modifiers.super_key() {
            return Vec::new();
        }
        match encode_key(&logical, &logical_unmod, self.modifiers, app_cursor) {
            Some(bytes) => vec![Effect::WriteToPty(bytes)],
            None => Vec::new(),
        }
    }

    /// Route a key to the open popup: Esc dismisses (settings gets the two-stage
    /// dirty-confirm), nav keys move the selection, Enter triggers the popup's
    /// action (apply backend / save settings / dismiss) and closes it.
    fn on_popup_key(&mut self, physical: PhysicalKey) -> Vec<Effect> {
        let PhysicalKey::Code(code) = physical else {
            return Vec::new();
        };
        if code == KeyCode::Escape {
            return self.close_popups_or_request();
        }
        if self.backend_switch.is_visible() {
            if let Some(intent) = input::backend_switch_nav(code) {
                self.backend_switch.apply(intent);
                return vec![Effect::Redraw];
            }
            if code == KeyCode::Enter {
                return vec![Effect::ApplyBackendSelection, Effect::ClosePopups, Effect::Redraw];
            }
        } else if self.history.is_visible() {
            if let Some(intent) = input::history_nav(code) {
                self.history.apply(intent);
                return vec![Effect::Redraw];
            }
            if code == KeyCode::Enter {
                return vec![Effect::ClosePopups, Effect::Redraw];
            }
        } else if self.settings.is_visible() {
            if let Some(intent) = input::settings_nav(code) {
                self.settings.apply(intent);
                return vec![Effect::Redraw];
            }
            if code == KeyCode::Enter {
                return vec![Effect::SaveSettings, Effect::ClosePopups, Effect::Redraw];
            }
        }
        Vec::new()
    }

    /// Construct the initial state. `session_id`/`start_time` are passed in
    /// (born at process start) so this stays deterministic and unit-testable.
    pub fn new(session_id: String, start_time: Instant, grid_size: (usize, usize)) -> Self {
        Self {
            grid_size,
            scroll: ScrollState::default(),
            scroll_velocity: None,
            modifiers: ModifiersState::empty(),
            cursor_pos: None,
            dragging_selection: false,
            selection: None,
            last_click: None,
            mouse_left_held: false,
            mouse_motion_cell: None,
            session_id,
            start_time,
            session_copied_until: None,
            backend_switch: BackendSwitchState::default(),
            history: HistoryDialogState::default(),
            settings: SettingsDialogState::default(),
            left: PanelManager::new(Policy::sidebar()),
            right: PanelManager::new(Policy::overlay()),
        }
    }

    /// True when any popup overlay is visible (gates input routing + mouse).
    pub fn any_popup_visible(&self) -> bool {
        self.backend_switch.is_visible()
            || self.history.is_visible()
            || self.settings.is_visible()
    }

    /// Close every visible popup (the state side; the caller requests redraw).
    pub fn close_all_popups(&mut self) {
        if self.backend_switch.is_visible() {
            self.backend_switch.apply(BackendSwitchIntent::Close);
        }
        if self.history.is_visible() {
            self.history.apply(HistoryIntent::Close);
        }
        if self.settings.is_visible() {
            self.settings.apply(SettingsIntent::Close);
        }
    }

    /// Arm the "Session ID copied!" flash until `deadline`.
    pub fn mark_session_copied(&mut self, deadline: Instant) {
        self.session_copied_until = Some(deadline);
    }

    /// Derived: is the copied-flash showing at frame time `now`? (R12)
    pub fn session_copied(&self, now: Instant) -> bool {
        self.session_copied_until.is_some_and(|deadline| now < deadline)
    }

    /// Derived: seconds since process start at frame time `now`. (R12)
    pub fn uptime_secs(&self, now: Instant) -> u64 {
        now.duration_since(self.start_time).as_secs()
    }

    /// Apply a wheel/trackpad delta. The caller MUST refresh the scroll bounds
    /// (`scroll.total_size_px`/`visible_px`) from the emulator + window first;
    /// this is pure on `AppState`. A new wheel event always interrupts in-flight
    /// momentum + any pending gesture-end fallback. A trackpad `Ended` kicks
    /// momentum immediately; a non-precise wheel arms the silence fallback.
    pub fn on_wheel(
        &mut self,
        dy: f32,
        phase: TouchPhase,
        precise: bool,
        now: Instant,
    ) -> Vec<Effect> {
        let mut fx = vec![Effect::CancelMomentum, Effect::CancelGestureEnd];
        self.scroll.scroll_by(dy);
        self.scroll_velocity =
            Some(ScrollVelocity::record(self.scroll_velocity, Vec2::new(0.0, dy), now));
        match phase {
            TouchPhase::Ended => fx.extend(self.on_gesture_end(now)),
            TouchPhase::Cancelled => self.scroll_velocity = None,
            TouchPhase::Started | TouchPhase::Moved => {
                if !precise {
                    fx.push(Effect::ScheduleGestureEnd);
                }
            }
        }
        fx.push(Effect::Redraw);
        fx
    }

    /// A gesture ended: kick momentum if the recorded velocity is fast enough,
    /// otherwise drop it. Pure on `AppState`.
    pub fn on_gesture_end(&mut self, now: Instant) -> Vec<Effect> {
        let Some(v) = self.scroll_velocity else {
            return Vec::new();
        };
        if v.velocity.length() < MOMENTUM_THRESHOLD {
            self.scroll_velocity = None;
            return Vec::new();
        }
        self.scroll_velocity = Some(ScrollVelocity {
            velocity: v.clamped_for_momentum(),
            last_update: now,
        });
        vec![Effect::ScheduleMomentum]
    }

    /// One momentum frame: decay the velocity, scroll by it, and stop once it
    /// falls below the cutoff. Caller refreshes scroll bounds first.
    pub fn on_momentum_tick(&mut self, now: Instant) -> Vec<Effect> {
        let Some(v) = self.scroll_velocity.as_mut() else {
            return Vec::new();
        };
        let elapsed = now.duration_since(v.last_update).as_secs_f32();
        v.last_update = now;
        v.velocity = decay_velocity(v.velocity, elapsed);
        if v.velocity.length() < MOMENTUM_MIN_VELOCITY {
            self.scroll_velocity = None;
            return vec![Effect::CancelMomentum];
        }
        let delta = v.velocity * elapsed;
        self.scroll.scroll_by(delta.y);
        vec![Effect::Redraw]
    }

    // ── selection (E.5) ──────────────────────────────────────────────────
    // The coordinator resolves window pixels → a `CellPoint` (via
    // `term_geometry::cell_at`) and supplies the emulator snapshot; these
    // methods own the selection STATE transition.

    /// Remember the latest mouse position (logical px). Used by the coordinator
    /// to start a drag-selection on the next press without re-querying winit.
    pub fn set_cursor_pos(&mut self, x: f32, y: f32) {
        self.cursor_pos = Some((x, y));
    }

    /// Record a click at `point` and return its multi-click count (1..=3),
    /// updating `last_click`. (Wraps the pure `term_geometry::next_click_count`.)
    pub fn next_click(&mut self, point: CellPoint, now: Instant, threshold_ms: u128) -> u32 {
        let count = crate::ui::term_geometry::next_click_count(
            self.last_click,
            point,
            now,
            threshold_ms,
        );
        self.last_click = Some(LastClick { time: now, point, count });
        count
    }

    /// Begin a selection at `point` for the given click `count`: 1 = linear
    /// (drag continues), 2 = word, 3 = line (both snap and end the drag).
    /// Word/line boundaries come from `snapshot`.
    pub fn begin_selection(&mut self, point: CellPoint, count: u32, snapshot: &RenderSnapshot) {
        match count {
            1 => {
                self.selection = Some(Selection::new(point));
                self.dragging_selection = true;
            }
            2 => {
                let (anchor, cursor) = expand_word(point, snapshot);
                self.selection = Some(Selection { anchor, cursor });
                self.dragging_selection = false;
            }
            _ => {
                let (anchor, cursor) = expand_line(point, snapshot);
                self.selection = Some(Selection { anchor, cursor });
                self.dragging_selection = false;
            }
        }
    }

    /// Extend the active drag-selection's cursor to `point`. Returns `true`
    /// when a drag was in flight (caller should redraw).
    pub fn drag_selection_to(&mut self, point: CellPoint) -> bool {
        if self.dragging_selection {
            if let Some(sel) = self.selection.as_mut() {
                sel.cursor = point;
                return true;
            }
        }
        false
    }

    /// End a left-drag. A click that didn't drag (anchor == cursor) clears the
    /// selection — keeps "click somewhere to deselect" working. Returns `true`
    /// when the selection was cleared (caller should redraw).
    pub fn end_selection_drag(&mut self) -> bool {
        if self.dragging_selection {
            self.dragging_selection = false;
            if self.selection.map(|s| s.is_empty()).unwrap_or(false) {
                self.selection = None;
                return true;
            }
        }
        false
    }
}
