//! Clipboard operations: copy the session id (with header flash), copy the
//! current selection, and paste the system clipboard into the PTY.

use std::time::Instant;

use term_clipboard::{
    get_image_filepaths_from_paths, pick_best_image, save_image_to_temp,
    should_insert_text_on_paste, ClipboardContent,
};
use term_gpu::{encode_paste, selection_to_text, shell_quote_path};

use crate::ui::gpu::chrome::SESSION_COPY_FLASH;

impl super::GpuApp {
    /// Copy the session UUID to the clipboard and trigger the
    /// header's "Session ID copied!" flash. Used by header click and
    /// the keyboard shortcut path (potentially later).
    pub(super) fn copy_session_id(&mut self) {
        self.clipboard
            .write(ClipboardContent::plain_text(self.state.session_id.clone()));
        self.state
            .mark_session_copied(Instant::now() + SESSION_COPY_FLASH);
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Copy the current selection to the system clipboard. Mirrors
    /// term_grid: `selection_to_text` against the current emulator
    /// snapshot → `ClipboardContent::plain_text`. Empty selections are
    /// skipped silently.
    pub(super) fn copy_selection(&mut self) {
        let Some(sel) = self.state.selection else { return };
        if sel.is_empty() {
            return;
        }
        let Some(emu) = self.session.emulator.as_ref() else { return };
        let snap = emu.snapshot();
        let text = selection_to_text(&sel, &snap);
        if text.is_empty() {
            return;
        }
        self.clipboard.write(ClipboardContent::plain_text(text));
    }

    /// Read the system clipboard and paste into the PTY. Mirrors
    /// Warp's `process_paste_event` step-for-step
    /// (`app/src/terminal/input.rs:10573`):
    ///
    ///   1. If `should_insert_text_on_paste(&content)` is true,
    ///      include `content.plain_text` in the payload.
    ///   2. Image filepaths in `content.paths` (filtered via
    ///      `get_image_filepaths_from_paths`) follow next — Claude
    ///      Code and other image-aware CLIs accept file paths as
    ///      input.
    ///   3. If `content.images` carries any pasteboard image data,
    ///      pick the highest-priority MIME from
    ///      `CLIPBOARD_IMAGE_MIME_TYPES`, save it to
    ///      `$TMPDIR/anyclaude_clipboard_<ts>.<ext>`, and append the
    ///      path to the payload.
    ///
    /// Paths are shell-quoted (single-quote escape) so spaces in
    /// names don't break tokenisation in the shell. The final
    /// payload is normalised (CRLF → LF) and wrapped in
    /// `\x1b[200~` … `\x1b[201~` when the emulator has bracketed
    /// paste enabled.
    pub(super) fn paste_into_pty(&mut self) {
        let content = self.clipboard.read();
        let mut parts: Vec<String> = Vec::new();

        if should_insert_text_on_paste(&content) && !content.plain_text.is_empty() {
            parts.push(content.plain_text.clone());
        }

        if let Some(paths) = content.paths.as_deref() {
            for path in get_image_filepaths_from_paths(paths) {
                parts.push(shell_quote_path(&path));
            }
        }

        if let Some(images) = content.images.as_deref() {
            if let Some(best) = pick_best_image(images) {
                if let Some(path) = save_image_to_temp(best, "anyclaude_clipboard") {
                    parts.push(shell_quote_path(&path));
                }
            }
        }

        if parts.is_empty() {
            return;
        }
        let payload = parts.join(" ");
        let bracketed = self
            .session.emulator
            .as_ref()
            .map(|e| e.bracketed_paste())
            .unwrap_or(false);
        let bytes = encode_paste(&payload, bracketed);
        if let Some(pty) = self.session.pty.as_mut() {
            if let Err(e) = pty.write(&bytes) {
                eprintln!("anyclaude: paste write failed: {e}");
            }
        }
    }
}
