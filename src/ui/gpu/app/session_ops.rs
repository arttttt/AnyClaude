//! Terminal session operations: draining the PTY's pending bytes into the
//! emulator, and tearing down + respawning the Claude session (Cmd+R).

use term_core::create_emulator;
use term_gpu::ScrollState;

use crate::ui::gpu::pty::ChildPty;

use super::{UserEvent, SCROLLBACK_LINES, SCROLL_BOTTOM_EPSILON};

impl super::GpuApp {
    /// Drain the PTY's pending bytes into the emulator. Returns true
    /// when at least one chunk arrived (caller should request redraw).
    /// Follow mode: if the scroll was at the bottom BEFORE applying
    /// the new bytes, re-pin to the bottom afterward so the cursor
    /// stays visible while the shell prints. Users who explicitly
    /// scrolled up keep position.
    pub(super) fn drain_pty(&mut self) -> bool {
        let Some(pty) = self.session.pty.as_mut() else {
            return false;
        };
        let chunks = pty.drain();
        if chunks.is_empty() {
            return false;
        }
        self.refresh_scroll_geometry();
        let was_at_bottom = self.state.scroll.offset_y <= SCROLL_BOTTOM_EPSILON;
        if let Some(emu) = self.session.emulator.as_mut() {
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

    /// Tear down the running Claude session and start a fresh one with
    /// the same spawn params. Wired to Cmd+R. The terminal state
    /// (emulator, scroll, selection) is reset so the new session
    /// renders into a clean panel.
    ///
    /// The old reader thread exits on its own as soon as its master
    /// PTY is dropped — the spawn flow is fire-and-forget.
    pub(super) fn restart_pty(&mut self) {
        self.session.pty = None;
        let (cols, rows) = self.state.grid_size;
        self.session.emulator = Some(create_emulator(cols, rows, SCROLLBACK_LINES));
        self.state.scroll = ScrollState::default();
        self.state.scroll_velocity = None;
        self.timers.cancel_momentum();
        self.timers.cancel_gesture_end();
        self.state.selection = None;
        self.state.dragging_selection = false;
        self.state.last_click = None;

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
                eprintln!("anyclaude: failed to restart shell: {e}");
            }
        }
        self.request_redraw();
    }
}
