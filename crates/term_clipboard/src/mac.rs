//! macOS clipboard backend via NSPasteboard.
//!
//! Adapted from the canonical objc2-app-kit example
//! (`objc2-app-kit-0.2.2/examples/nspasteboard.rs`) and Warp's
//! `crates/warpui/src/platform/mac/clipboard.rs` (MIT).
//!
//! Plain-text only in this commit; HTML, images, and file paths
//! land in follow-up commits but the public API doesn't change —
//! [`MacClipboard`] always reads / writes a [`ClipboardContent`].

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::{NSArray, NSCopying, NSString};

use crate::{Clipboard, ClipboardContent};

/// Backed by the system general pasteboard.
pub struct MacClipboard {
    pasteboard: Retained<NSPasteboard>,
}

// NSPasteboard's generalPasteboard is documented as safe to access
// from any thread. We hold a Retained reference (refcounted via
// objc2), so the inner pointer outlives the App's lifetime.
unsafe impl Send for MacClipboard {}

impl MacClipboard {
    /// Acquire a handle to the system general pasteboard. Cheap —
    /// `+[NSPasteboard generalPasteboard]` returns a long-lived
    /// singleton; we just retain a strong reference.
    pub fn new() -> Self {
        let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
        Self { pasteboard }
    }
}

impl Default for MacClipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard for MacClipboard {
    fn write(&mut self, contents: ClipboardContent) {
        // No-op when the payload carries nothing the macOS pasteboard
        // can hold — avoids stomping the existing clipboard with an
        // empty NSString.
        if contents.plain_text.is_empty() {
            return;
        }
        let ns = NSString::from_str(&contents.plain_text);
        // `NSString` conforms to `NSPasteboardWriting`; passing it
        // through `ProtocolObject` lets us put it into the NSArray
        // that `writeObjects:` expects.
        let obj = ProtocolObject::from_retained(ns.copy());
        let array = NSArray::from_vec(vec![obj]);
        unsafe {
            let _ = self.pasteboard.clearContents();
            let _ = self.pasteboard.writeObjects(&array);
        }
    }

    fn read(&mut self) -> ClipboardContent {
        let text = unsafe { self.pasteboard.stringForType(NSPasteboardTypeString) };
        match text {
            Some(s) => ClipboardContent::plain_text(s.to_string()),
            None => ClipboardContent::default(),
        }
    }
}
