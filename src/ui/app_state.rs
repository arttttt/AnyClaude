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

use term_gpu::{ScrollState, ScrollVelocity, Selection};
use winit::keyboard::ModifiersState;

use crate::ui::backend_switch::{BackendSwitchIntent, BackendSwitchState};
use crate::ui::history::{HistoryDialogState, HistoryIntent};
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
}

impl AppState {
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
            session_id,
            start_time,
            session_copied_until: None,
            backend_switch: BackendSwitchState::default(),
            history: HistoryDialogState::default(),
            settings: SettingsDialogState::default(),
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
}
