//! `ChildPtySpawner` — builds a [`TerminalSurface`] (VT emulator + PTY child)
//! from a [`ChildSpec`]. The single place that launches a teammate's process;
//! `ChildSessionManager` decides WHEN (on `Register`), this decides HOW. Ported
//! from `term_grid.rs`'s `spawn_panel`.

use std::io;

use term_core::create_emulator;

use crate::ui::child_session::ChildSpec;
use crate::ui::gpu::pty::ChildPty;
use crate::ui::gpu::surface::TerminalSurface;

pub(super) struct ChildPtySpawner {
    /// Scrollback lines for each pane's emulator.
    scrollback: usize,
}

impl ChildPtySpawner {
    pub(super) fn new(scrollback: usize) -> Self {
        Self { scrollback }
    }

    /// Spawn `spec.command` into a fresh PTY + emulator sized `(cols, rows)`.
    /// `on_data` fires from the reader thread after each read — the caller tags it
    /// with the pane's id so the right surface is drained.
    pub(super) fn spawn<F>(
        &self,
        spec: &ChildSpec,
        cols: usize,
        rows: usize,
        on_data: F,
    ) -> io::Result<TerminalSurface>
    where
        F: Fn() + Send + 'static,
    {
        let emulator = create_emulator(cols, rows, self.scrollback);
        let pty = ChildPty::spawn(
            cols as u16,
            rows as u16,
            spec.command.clone(),
            spec.args.clone(),
            spec.env.clone(),
            on_data,
        )?;
        Ok(TerminalSurface::new(emulator, pty, (cols, rows)))
    }
}
