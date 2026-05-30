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
    /// loop — every winit / user event funnels through here. (Mouse press builds
    /// its own ctx carrying the emulator snapshot; see `on_mouse_press`.)
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
                Effect::WriteToPty(bytes) => {
                    if let Some(pty) = self.session.pty.as_mut() {
                        if let Err(e) = pty.write(&bytes) {
                            eprintln!("anyclaude: PTY write failed: {e}");
                        }
                    }
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
                let (precise, dy) = match delta {
                    MouseScrollDelta::PixelDelta(p) => (true, p.y as f32),
                    MouseScrollDelta::LineDelta(_, v) => (false, v * NUM_PIXELS_PER_LINE),
                };
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
                // Resolve the cell under the cursor only mid-drag (it reads the
                // emulator snapshot — skip the cost otherwise).
                let point = if self.state.dragging_selection {
                    self.cell_at(lx, ly)
                } else {
                    None
                };
                self.dispatch(Msg::CursorMoved { x: lx, y: ly, point });
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
                // since the event loop is the coordinator's to drive.
                if self.dispatch(Msg::Key {
                    logical: event.logical_key,
                    physical: event.physical_key,
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
