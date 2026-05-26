//! macOS-only smoke tests against the real NSPasteboard.
//!
//! These tests overwrite the system clipboard, so they're gated
//! behind `#[ignore]` to keep `cargo test` from trashing whatever
//! the user has on their clipboard. Run explicitly with:
//!
//! ```bash
//! cargo test -p term_clipboard --test mac_smoke -- --ignored
//! ```
//!
//! All scenarios live inside a single `#[test]` so cargo runs them
//! sequentially. Parallel NSPasteboard access from multiple test
//! threads (each one not the main thread) reliably SIGSEGVs.

#![cfg(target_os = "macos")]

use term_clipboard::{Clipboard, ClipboardContent, MacClipboard};

#[test]
#[ignore = "writes to the real NSPasteboard"]
fn mac_clipboard_round_trips() {
    let mut cb = MacClipboard::new();

    // Plain text round-trip.
    cb.write(ClipboardContent::plain_text("term_clipboard test".into()));
    assert_eq!(cb.read().plain_text, "term_clipboard test");

    // Writing an empty payload is a no-op — the previous text stays.
    cb.write(ClipboardContent::default());
    assert_eq!(cb.read().plain_text, "term_clipboard test");

    // Unicode survives.
    cb.write(ClipboardContent::plain_text("привет 🦀".into()));
    assert_eq!(cb.read().plain_text, "привет 🦀");
}
