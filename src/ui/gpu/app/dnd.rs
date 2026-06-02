//! Drag-and-drop: a file (or image) dropped on the window is inserted into the
//! focused PTY as its raw path — the standard terminal DnD gesture.
//!
//! winit (macOS) delivers one `WindowEvent::DroppedFile` per file and carries no
//! drop position, so each drop is routed to the keyboard-focused target (the
//! main session or the focused teammate pane, via `write_to_focused`) and
//! inserted as the path plus a trailing space. Several files in one drag arrive
//! as consecutive events, so they land space-separated.
//!
//! The path is inserted RAW (no quoting / escaping): the drop target is Claude
//! Code's text prompt, not a shell parsing arguments, so the literal path is
//! exactly what reads back the dropped file — quotes or backslash escapes would
//! only corrupt it. (Warp escapes because it types into a shell command line; we
//! don't.) Images need no special case: Claude Code reads a dropped image
//! straight from its path.

use std::path::PathBuf;

impl super::GpuApp {
    /// Insert a dropped file's path into the focused PTY: the raw path plus a
    /// trailing space, typed as input. Empty and non-UTF-8 paths are skipped.
    pub(super) fn on_file_dropped(&mut self, path: PathBuf) {
        let Some(path) = path.to_str() else { return };
        if path.is_empty() {
            return;
        }
        self.write_to_focused(format!("{path} ").as_bytes());
    }
}
