//! `TerminalSurface` — one teammate pane's live terminal: its VT emulator + PTY
//! child + the last grid size it was sized to. The per-pane analogue of the main
//! [`Session`](super::session::Session); a `ChildPtySpawner` builds these and the
//! `Panes` collaborator owns them keyed by `PaneId`.

use term_core::TerminalEmulator;

use crate::ui::gpu::pty::ChildPty;

pub(super) struct TerminalSurface {
    pub(super) emulator: Box<dyn TerminalEmulator>,
    pub(super) pty: ChildPty,
    /// Last `(cols, rows)` the emulator + PTY were sized to — lets `resize` skip
    /// redundant work (the term_grid lesson).
    grid_size: (usize, usize),
}

impl TerminalSurface {
    pub(super) fn new(
        emulator: Box<dyn TerminalEmulator>,
        pty: ChildPty,
        grid_size: (usize, usize),
    ) -> Self {
        Self { emulator, pty, grid_size }
    }

    /// Drain queued PTY bytes into the emulator. Returns whether any arrived (the
    /// caller requests a redraw).
    pub(super) fn drain(&mut self) -> bool {
        let chunks = self.pty.drain();
        if chunks.is_empty() {
            return false;
        }
        for chunk in chunks {
            self.emulator.process(&chunk);
        }
        true
    }

    /// Resize the emulator + PTY to `(cols, rows)`, skipping when unchanged.
    pub(super) fn resize(&mut self, cols: usize, rows: usize) {
        if self.grid_size == (cols, rows) || cols == 0 || rows == 0 {
            return;
        }
        self.emulator.resize(cols, rows);
        self.pty.resize(cols as u16, rows as u16);
        self.grid_size = (cols, rows);
    }
}
