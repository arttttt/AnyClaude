//! The event loop: the `Msg` → `apply` → `Effect` cycle and the winit
//! `ApplicationHandler` impl.
//!
//! `dispatch` is the single coordinator-side entry — every winit / user event
//! funnels through it into the pure `AppState::apply`, and `perform_effects` is
//! the one place a state transition reaches a resource (timers, PTY, clipboard,
//! renderer, popups). `ApplicationHandler` translates raw winit events into
//! `Msg`s (resolving the read-only resource gates the reducer can't see).

use std::sync::Arc;
use std::time::Instant;

use term_core::create_emulator;
use term_gpu::{
    GpuRenderer, MouseButton, MouseEventKind, GESTURE_END_TIMEOUT, MOMENTUM_FRAME_INTERVAL,
    NUM_PIXELS_PER_LINE,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{WindowAttributes, WindowId};

use crate::ui::app_state::{ApplyCtx, Effect, Msg};
use crate::ui::gpu::diagnostic;
use crate::ui::gpu::pty::ChildPty;

use super::{UserEvent, INITIAL_H, INITIAL_W, MULTI_CLICK_THRESHOLD_MS, SCROLLBACK_LINES};

impl super::GpuApp {
    /// Translate a `Msg` to its state transition and perform the resulting
    /// effects: build the read-only `ApplyCtx`, call `AppState::apply`, then run
    /// each `Effect`. This is the single coordinator-side entry for the event
    /// loop — every winit / user event funnels through here. `snapshot` is `None`
    /// because only selection word/line-expansion needs the grid content; the
    /// mouse-press path builds its own ctx that carries the snapshot (the
    /// two-entry seam — see `on_mouse_press`), so the common path avoids cloning
    /// it per keystroke / tick.
    pub(super) fn dispatch(&mut self, msg: Msg) -> bool {
        let ctx = ApplyCtx {
            now: Instant::now(),
            snapshot: None,
            multi_click_threshold_ms: MULTI_CLICK_THRESHOLD_MS,
        };
        let effects = self.state.apply(msg, &ctx);
        self.perform_effects(effects)
    }

    /// Perform the side effects `apply` returned. The one place a state
    /// transition reaches a resource — timers, redraw, PTY / clipboard /
    /// renderer / popups; the reducer stayed pure on `AppState` (bucket 3-S).
    /// Returns `true` when an effect asked the app to exit (`Quit`), which the
    /// coordinator turns into `event_loop.exit()` (it owns the event loop).
    pub(super) fn perform_effects(&mut self, effects: Vec<Effect>) -> bool {
        let mut exit = false;
        for effect in effects {
            match effect {
                Effect::CancelMomentum => self.timers.cancel_momentum(),
                Effect::CancelGestureEnd => self.timers.cancel_gesture_end(),
                Effect::ScheduleMomentum => {
                    self.timers.schedule_momentum(&self.proxy, MOMENTUM_FRAME_INTERVAL);
                }
                Effect::ScheduleGestureEnd => {
                    self.timers.schedule_gesture_end(&self.proxy, GESTURE_END_TIMEOUT);
                }
                Effect::Redraw => self.request_redraw(),
                Effect::ResizeEmulatorAndPty { cols, rows } => {
                    if let Some(emu) = self.session.emulator.as_mut() {
                        emu.resize(cols, rows);
                    }
                    if let Some(pty) = self.session.pty.as_ref() {
                        pty.resize(cols as u16, rows as u16);
                    }
                }
                Effect::WriteToPty(bytes) => self.write_to_main(&bytes),
                Effect::WriteToFocused(bytes) => self.write_to_focused(&bytes),
                Effect::ToggleInputFocus => {
                    self.state.toggle_input_focus();
                    self.request_redraw();
                }
                Effect::ToggleBackendPopup => self.toggle_backend_switch_popup(),
                Effect::ToggleHistoryPopup => self.toggle_history_popup(),
                Effect::ToggleSettingsPopup => self.toggle_settings_popup(),
                Effect::ClosePopups => self.state.close_all_popups(),
                Effect::ApplyBackendSelection => self.apply_backend_switch_selection(),
                Effect::SaveSettings => self.apply_settings_and_save(),
                Effect::CopySelection => self.copy_selection(),
                Effect::CopySessionId => self.copy_session_id(),
                Effect::Paste => self.paste_into_pty(),
                Effect::RestartPty => self.restart_pty(),
                Effect::DumpDiagnostic => self.dump_diagnostic(),
                Effect::DebugTogglePanels => self.debug_toggle_panels(),
                Effect::DebugUnregisterPane => self.debug_unregister_focused_pane(),
                Effect::PagePrev => self.page_panel(false),
                Effect::PageNext => self.page_panel(true),
                Effect::Quit => exit = true,
                Effect::Drain => {
                    if self.drain_pty() {
                        self.request_redraw();
                    }
                }
            }
        }
        exit
    }

    /// React to one teammate-session lifecycle event: the [`ChildSessionManager`]
    /// updates identity + `state.right` (UI), and the coordinator orchestrates the
    /// matching resources — spawn a [`TerminalSurface`] into `panes` on `Register`,
    /// drop it on `Unregister`. The single coordinator entry point for
    /// `ChildSessionEvent`s (debug emitter today, `TmuxAdapter` later).
    fn apply_child_session_event(&mut self, event: crate::ui::child_session::ChildSessionEvent) {
        use crate::ui::child_session::ChildSessionEvent;
        // Pull out what the resource side needs before the event is consumed.
        let spec = match &event {
            ChildSessionEvent::Register(s) => Some(s.clone()),
            ChildSessionEvent::Unregister(_) => None,
        };
        let closing = match &event {
            ChildSessionEvent::Unregister(p) => Some(*p),
            ChildSessionEvent::Register(_) => None,
        };

        let new_pane = self.child_sessions.apply(event, &mut self.state.right);

        if let (Some(spec), Some(pane)) = (spec, new_pane) {
            let (cols, rows) = super::INITIAL_PANE_GRID;
            let proxy = self.proxy.clone();
            let on_data = move || {
                let _ = proxy.send_event(UserEvent::PtyBytes(pane));
            };
            if let Err(e) = self.panes.spawn(pane, &spec, cols, rows, on_data) {
                eprintln!("anyclaude: teammate pane spawn failed: {e}");
            }
        }
        if let Some(pane) = closing {
            self.panes.remove(pane);
        }
        self.request_redraw();
    }

    /// Debug-only (Ctrl+P): the first hit REGISTERS a few mock teammate sessions
    /// (through `ChildSessionEvent`s — exercising the real registry → panel flow,
    /// just with no process behind them yet), then toggle the overlay. Real
    /// teammates arrive when the `TmuxAdapter` produces the same events.
    fn debug_toggle_panels(&mut self) {
        use crate::ui::child_session::{ChildSessionEvent, ChildSpec};
        if self.child_sessions.is_empty() {
            // Each mock runs an interactive shell so the pane shows a live grid
            // (real teammates run `claude` via the control plane). Accent colours
            // echo Claude Code's teammate palette.
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
            let env = vec![("TERM".to_string(), "xterm-256color".to_string())];
            let mocks = [
                ("module-mapper", [0.30, 0.55, 0.95, 1.0]),
                ("flow-tracer", [0.35, 0.80, 0.45, 1.0]),
                ("deps-mapper", [0.90, 0.75, 0.30, 1.0]),
                ("type-checker", [0.80, 0.45, 0.85, 1.0]),
                ("test-runner", [0.95, 0.50, 0.40, 1.0]),
                ("doc-writer", [0.45, 0.75, 0.85, 1.0]),
            ];
            for (name, accent) in mocks {
                let spec = ChildSpec {
                    name: name.to_string(),
                    accent,
                    command: shell.clone(),
                    args: vec![],
                    env: env.clone(),
                };
                self.apply_child_session_event(ChildSessionEvent::Register(spec));
            }
        }
        self.state.right.toggle();
        self.request_redraw();
    }

    /// Debug-only (Ctrl+K): UNREGISTER the focused teammate's session — proves the
    /// `Unregister` lifecycle live (panel removed, focus falls back). A no-op when
    /// the focused panel isn't a registered child.
    fn debug_unregister_focused_pane(&mut self) {
        use crate::ui::child_session::ChildSessionEvent;
        let Some(panel) = self.state.right.focus() else { return };
        let Some(pane) = self.child_sessions.pane_for(panel) else { return };
        self.apply_child_session_event(ChildSessionEvent::Unregister(pane));
    }

    /// Page the right teammates overlay forward / back (⌥→ / ⌥←): move its focus
    /// one panel; the pager's `page_scroll` tween then slides to it. Coordinator-
    /// side state mutation like `debug_toggle_panels`; a no-op when empty.
    fn page_panel(&mut self, forward: bool) {
        if forward {
            self.state.right.focus_next();
        } else {
            self.state.right.focus_prev();
        }
        self.request_redraw();
    }

    /// The pane behind the focused teammate panel, if any — the keyboard target
    /// when input is routed to the overlay. `pub(super)` so the paste path
    /// (clipboard module) resolves the same target.
    pub(super) fn focused_pane(&self) -> Option<crate::ui::child_session::PaneId> {
        let panel = self.state.right.focus()?;
        self.child_sessions.pane_for(panel)
    }

    /// Write `bytes` to whichever terminal holds keyboard focus: the focused
    /// teammate pane when input is routed to the overlay (`input_on_teammates`),
    /// otherwise the main session. Falls back to the main session if the focused
    /// pane has vanished (so a keystroke is never silently dropped).
    pub(super) fn write_to_focused(&mut self, bytes: &[u8]) {
        if self.state.input_on_teammates() {
            if let Some(pane) = self.focused_pane() {
                if let Some(surface) = self.panes.get_mut(pane) {
                    if let Err(e) = surface.write(bytes) {
                        eprintln!("anyclaude: teammate PTY write failed: {e}");
                    }
                    return;
                }
            }
        }
        self.write_to_main(bytes);
    }

    /// Write `bytes` to the main session's PTY — the default keyboard target and
    /// the sink for mouse reports (always the main grid under the cursor).
    pub(super) fn write_to_main(&mut self, bytes: &[u8]) {
        if let Some(pty) = self.session.pty.as_mut() {
            if let Err(e) = pty.write(bytes) {
                eprintln!("anyclaude: PTY write failed: {e}");
            }
        }
    }

    /// Dump a diagnostic snapshot (grid + scroll + emulator) to stderr.
    fn dump_diagnostic(&self) {
        let snap = self.session.emulator.as_ref().map(|e| e.snapshot());
        diagnostic::dump_snapshot(
            self.state.grid_size,
            self.state.scroll.offset_y,
            self.state.scroll.max_offset(),
            snap.as_ref(),
        );
    }
}

impl ApplicationHandler<UserEvent> for super::GpuApp {
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
        self.session.emulator = Some(create_emulator(cols, rows, SCROLLBACK_LINES));

        let proxy = self.proxy.clone();
        match ChildPty::spawn(
            cols as u16,
            rows as u16,
            self.session.spawn_command.clone(),
            self.session.spawn_args.clone(),
            self.session.spawn_env.clone(),
            move || {
                let _ = proxy.send_event(UserEvent::PtyBytesArrived);
            },
        ) {
            Ok(pty) => {
                self.session.pty = Some(pty);
            }
            Err(e) => {
                eprintln!("anyclaude: failed to spawn shell: {e}");
                event_loop.exit();
                return;
            }
        }

        self.timers.start_periodic(&self.proxy);

        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyBytesArrived => {
                self.dispatch(Msg::PtyBytes);
            }
            UserEvent::PtyBytes(pane) => {
                if self.panes.drain(pane) {
                    self.request_redraw();
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
                self.dispatch(Msg::Tick);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                if self.dispatch(Msg::Close) {
                    event_loop.exit();
                }
            }
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
                self.text.cell_metrics = None;
                self.resync_grid();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.dispatch(Msg::ModifiersChanged(mods.state()));
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let (precise, dx, dy) = match delta {
                    MouseScrollDelta::PixelDelta(p) => (true, p.x as f32, p.y as f32),
                    MouseScrollDelta::LineDelta(h, v) => {
                        (false, h * NUM_PIXELS_PER_LINE, v * NUM_PIXELS_PER_LINE)
                    }
                };
                // Over the teammates overlay, a horizontal two-finger swipe pages
                // it (page_swipe sorts horizontal from vertical and the gesture
                // boundary); the wheel doesn't reach the terminal underneath.
                if self.cursor_over_overlay() {
                    self.page_swipe(dx, dy, phase);
                    return;
                }
                // A mouse-reporting app gets the wheel as button 64 / 65 instead
                // of scrolling our scrollback (§6).
                let wheel = if dy > 0.0 { MouseButton::WheelUp } else { MouseButton::WheelDown };
                let mouse_report = self.mouse_report_at_cursor(wheel, MouseEventKind::Press);
                if mouse_report.is_none() {
                    self.refresh_scroll_geometry();
                }
                self.dispatch(Msg::Wheel { dy, phase, precise, mouse_report });
            }
            WindowEvent::CursorMoved { position, .. } => {
                let PhysicalPosition { x, y } = position;
                let sf = self.scale_factor.max(0.0001);
                let (lx, ly) = (x as f32 / sf, y as f32 / sf);
                // Hover cursor: resize over a panel edge, pointer over the pill.
                self.update_hover_cursor(lx, ly);
                // A panel-edge drag owns cursor motion: the overlay hugs the
                // window's right edge, so the dragged width is `right - cursor_x`.
                if let Some(mgr) = self.state.panel_edge_drag {
                    let win_w = self.window.as_ref().map(|w| w.inner_size().width as f32 / sf);
                    if let Some(win_w) = win_w {
                        self.dispatch(Msg::PanelResize { mgr, width: win_w - lx });
                    }
                    return;
                }
                // Resolve the cell when a selection drag is in flight OR a
                // mouse-reporting app wants motion (both read the emulator
                // snapshot — skip the cost otherwise).
                let reports_motion = self
                    .session
                    .emulator
                    .as_ref()
                    .map(|e| e.mouse_protocol().reports_motion())
                    .unwrap_or(false);
                let point = if self.state.dragging_selection || reports_motion {
                    self.cell_at(lx, ly)
                } else {
                    None
                };
                let motion_report = if reports_motion { self.motion_report(point) } else { None };
                self.dispatch(Msg::CursorMoved { x: lx, y: ly, point, motion_report });
            }
            WindowEvent::MouseInput {
                state,
                button: WinitMouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.on_mouse_press(),
                ElementState::Released => {
                    let mouse_report =
                        self.mouse_report_at_cursor(MouseButton::Left, MouseEventKind::Release);
                    self.dispatch(Msg::MouseRelease { mouse_report });
                }
            },
            // Middle / right buttons have no local action — they only matter to a
            // mouse-reporting app, so forward the encoded report when one's active
            // and otherwise drop the event (§6).
            WindowEvent::MouseInput {
                state,
                button: button @ (WinitMouseButton::Middle | WinitMouseButton::Right),
                ..
            } => {
                let report_button = if matches!(button, WinitMouseButton::Right) {
                    MouseButton::Right
                } else {
                    MouseButton::Middle
                };
                let kind = match state {
                    ElementState::Pressed => MouseEventKind::Press,
                    ElementState::Released => MouseEventKind::Release,
                };
                if let Some(bytes) = self.mouse_report_at_cursor(report_button, kind) {
                    self.dispatch(Msg::MouseReport(bytes));
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed =>
            {
                // All key routing — popup nav while a popup is open, Cmd/Super
                // app shortcuts, otherwise a terminal key encoded to the PTY —
                // lives in AppState::apply. Quit comes back as the exit signal,
                // since the event loop is the coordinator's to drive. Resolve the
                // resource-backed inputs the encoder needs here: the DECCKM state
                // (SS3 vs CSI arrows) and the un-composed base key (Meta form).
                // The DECCKM is read from the FOCUSED terminal — a focused
                // teammate's arrows encode against its own mode, not the main's.
                let app_cursor = if self.state.input_on_teammates() {
                    self.focused_pane()
                        .and_then(|pane| self.panes.get(pane))
                        .map(|s| s.app_cursor())
                        .unwrap_or(false)
                } else {
                    self.session
                        .emulator
                        .as_ref()
                        .map(|e| e.cursor_keys_app())
                        .unwrap_or(false)
                };
                let logical_unmod = key_without_modifiers(&event);
                if self.dispatch(Msg::Key {
                    logical: event.logical_key,
                    logical_unmod,
                    physical: event.physical_key,
                    app_cursor,
                }) {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }
}

/// The layout-resolved key WITHOUT modifiers applied. On macOS this strips the
/// Option composition (so `Option+a` is the base `a`, not `å`), which
/// `encode_key` uses for the Meta / ESC-prefix form. Other platforms fall back
/// to the logical key (anyclaude is macOS-targeted).
fn key_without_modifiers(event: &winit::event::KeyEvent) -> winit::keyboard::Key {
    #[cfg(target_os = "macos")]
    {
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        event.key_without_modifiers()
    }
    #[cfg(not(target_os = "macos"))]
    {
        event.logical_key.clone()
    }
}
