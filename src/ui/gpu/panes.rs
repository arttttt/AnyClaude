//! `Panes` — the coordinator's store of teammate [`TerminalSurface`]s, keyed by
//! `PaneId` (bucket 3-T resources, mirroring the single-pane [`Session`](
//! super::session::Session)). Owns the [`ChildPtySpawner`]; the coordinator
//! drives it from `ChildSessionEvent` handling (spawn on register, remove on
//! unregister) and the per-pane `PtyBytes` wake (drain). Heavy resources live
//! here, NOT in `ChildSessionManager` (which keeps only identity).

use std::collections::HashMap;
use std::io;

use crate::ui::child_session::{ChildSpec, PaneId};
use crate::ui::gpu::spawn::ChildPtySpawner;
use crate::ui::gpu::surface::TerminalSurface;

pub(super) struct Panes {
    surfaces: HashMap<PaneId, TerminalSurface>,
    spawner: ChildPtySpawner,
}

impl Panes {
    pub(super) fn new(scrollback: usize) -> Self {
        Self { surfaces: HashMap::new(), spawner: ChildPtySpawner::new(scrollback) }
    }

    /// Spawn a surface for `pane` from `spec` (at `cols × rows`) and store it.
    /// `on_data` (built by the coordinator with the pane id) fires on each PTY
    /// read.
    pub(super) fn spawn<F>(
        &mut self,
        pane: PaneId,
        spec: &ChildSpec,
        cols: usize,
        rows: usize,
        on_data: F,
    ) -> io::Result<()>
    where
        F: Fn() + Send + 'static,
    {
        let surface = self.spawner.spawn(spec, cols, rows, on_data)?;
        self.surfaces.insert(pane, surface);
        Ok(())
    }

    /// Drop `pane`'s surface (its reader thread exits when the PTY master drops).
    pub(super) fn remove(&mut self, pane: PaneId) {
        self.surfaces.remove(&pane);
    }

    /// Drain `pane`'s pending PTY bytes into its emulator. Returns whether any
    /// arrived. No-op for an unknown pane.
    pub(super) fn drain(&mut self, pane: PaneId) -> bool {
        self.surfaces.get_mut(&pane).map(|s| s.drain()).unwrap_or(false)
    }

    /// Mutable access to a pane's surface (the renderer resizes it to its page
    /// rect and reads its emulator snapshot).
    pub(super) fn get_mut(&mut self, pane: PaneId) -> Option<&mut TerminalSurface> {
        self.surfaces.get_mut(&pane)
    }
}
