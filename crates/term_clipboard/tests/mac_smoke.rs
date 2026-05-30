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

use term_clipboard::{Clipboard, ClipboardContent, ImageData, MacClipboard};

// One-pixel PNG (8 bytes signature + minimal IHDR + IDAT + IEND).
// Just enough for the macOS pasteboard to accept the data.
const PNG_BYTES: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
    0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR length + tag
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1×1
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, // depth, colour, etc.
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, // IDAT length + tag
    0x54, 0x78, 0x9C, 0x62, 0x00, 0x01, 0x00, 0x00, // compressed pixel
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, // ...
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, // IEND
    0x42, 0x60, 0x82,
];

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

    // HTML rides alongside plain text on the same pasteboard item.
    cb.write(ClipboardContent {
        plain_text: "hello".into(),
        html: Some("<b>hello</b>".into()),
        ..Default::default()
    });
    let out = cb.read();
    assert_eq!(out.plain_text, "hello");
    assert_eq!(out.html.as_deref(), Some("<b>hello</b>"));

    // Image data round-trips. macOS may rewrite the bytes (NSImage
    // re-encoding through pasteboard helpers), so we only check
    // that the same MIME type comes back and the payload is non-empty.
    cb.write(ClipboardContent {
        plain_text: "caption".into(),
        images: Some(vec![ImageData {
            data: PNG_BYTES.to_vec(),
            mime_type: "image/png".into(),
            filename: None,
        }]),
        ..Default::default()
    });
    let out = cb.read();
    assert_eq!(out.plain_text, "caption");
    let imgs = out.images.expect("expected image data on pasteboard");
    assert!(!imgs.is_empty(), "image vec should be non-empty");
    assert!(
        imgs.iter().any(|i| i.mime_type == "image/png"),
        "expected at least one image/png entry"
    );
}
