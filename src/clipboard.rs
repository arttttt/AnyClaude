//! Clipboard access for text copy/paste (e.g. mouse selection).

use arboard::Clipboard;

/// Handler for clipboard operations.
pub struct ClipboardHandler {
    clipboard: Clipboard,
}

impl ClipboardHandler {
    /// Create a new clipboard handler.
    pub fn new() -> Result<Self, arboard::Error> {
        let clipboard = Clipboard::new()?;
        Ok(Self { clipboard })
    }

    /// Write text to the system clipboard.
    pub fn set_text(&mut self, text: &str) -> Result<(), String> {
        self.clipboard
            .set_text(text.to_string())
            .map_err(|e| format!("Failed to set clipboard text: {}", e))
    }
}
