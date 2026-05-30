//! The terminal session resources: the PTY child, the VT emulator, and the
//! spawn params used to (re)launch Claude Code. Bundled so `GpuApp` isn't
//! littered with the handles; the coordinator's `drain_pty` / `restart_pty`
//! orchestrate this against `AppState` (follow-mode scroll, reset on restart)
//! and the timers. `pty` / `emulator` are lazily populated in `resumed` (they
//! need the window's pixel size first).

use term_core::TerminalEmulator;

use crate::ui::gpu::pty::ChildPty;

pub(super) struct Session {
    pub(super) pty: Option<ChildPty>,
    pub(super) emulator: Option<Box<dyn TerminalEmulator>>,
    /// Spawn params, prepared by `run()` before the event loop; reused by
    /// `resumed` + `restart_pty`.
    pub(super) spawn_command: String,
    pub(super) spawn_args: Vec<String>,
    pub(super) spawn_env: Vec<(String, String)>,
}

impl Session {
    pub(super) fn new(
        spawn_command: String,
        spawn_args: Vec<String>,
        spawn_env: Vec<(String, String)>,
    ) -> Self {
        Self { pty: None, emulator: None, spawn_command, spawn_args, spawn_env }
    }
}
