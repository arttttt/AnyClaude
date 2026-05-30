//! macOS clipboard backend via NSPasteboard.
//!
//! Adapted from the canonical objc2-app-kit example
//! (`objc2-app-kit-0.2.2/examples/nspasteboard.rs`) and Warp's
//! `crates/warpui/src/platform/mac/clipboard.rs` (MIT). Full
//! ClipboardContent parity: plain text, HTML, file paths, and
//! images of every MIME type Warp's mac backend supports
//! (PNG / JPEG / GIF / WebP / SVG).

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
use objc2::ClassType;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString};
use objc2_foundation::{NSArray, NSCopying, NSData, NSString, NSURL};

use crate::{Clipboard, ClipboardContent, ImageData};

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
        // No-op when the payload carries nothing the macOS
        // pasteboard could hold — avoids stomping the existing
        // clipboard with an empty NSString.
        if contents.is_empty() {
            return;
        }

        // `writeObjects` clears and replaces. We use it for the
        // plain-text portion (NSString conforms to
        // NSPasteboardWriting), then `addTypes_owner` +
        // `setString_forType` / `setData_forType` to layer HTML and
        // images on the same pasteboard item.
        if !contents.plain_text.is_empty() {
            let ns = NSString::from_str(&contents.plain_text);
            let obj = ProtocolObject::from_retained(ns.copy());
            let array = NSArray::from_vec(vec![obj]);
            unsafe {
                let _ = self.pasteboard.clearContents();
                let _ = self.pasteboard.writeObjects(&array);
            }
        } else {
            // Caller wants to publish non-text content. Still need
            // to clear what's there so the new types replace the old.
            unsafe {
                let _ = self.pasteboard.clearContents();
            }
        }

        if let Some(html) = contents.html.as_deref() {
            let ns_type: &NSString = unsafe { NSPasteboardTypeHTML };
            let ns_html = NSString::from_str(html);
            unsafe {
                let _ = self.pasteboard.addTypes_owner(
                    &NSArray::from_vec(vec![ns_type.copy()]),
                    None,
                );
                let _ = self.pasteboard.setString_forType(&ns_html, ns_type);
            }
        }

        if let Some(images) = contents.images.as_ref() {
            for image in images {
                let Some(utf) = mime_to_pasteboard_type(&image.mime_type) else {
                    continue;
                };
                let pb_type = NSString::from_str(utf);
                let ns_data = NSData::with_bytes(&image.data);
                unsafe {
                    let _ = self.pasteboard.addTypes_owner(
                        &NSArray::from_vec(vec![pb_type.copy()]),
                        None,
                    );
                    let _ = self
                        .pasteboard
                        .setData_forType(Some(&ns_data), &pb_type);
                }
            }
        }
    }

    fn read(&mut self) -> ClipboardContent {
        let plain_text = unsafe { self.pasteboard.stringForType(NSPasteboardTypeString) }
            .map(|s| s.to_string())
            .unwrap_or_default();

        let html = unsafe { self.pasteboard.stringForType(NSPasteboardTypeHTML) }
            .map(|s| s.to_string());

        let paths = read_file_paths(&self.pasteboard);
        let images = read_images(&self.pasteboard);

        ClipboardContent {
            plain_text,
            paths,
            html,
            images,
        }
    }
}

/// MIME → NSPasteboard UTI for image writes. Matches Warp's mapping
/// at `crates/warpui/src/platform/mac/clipboard.rs::pasteboard_type_for_image_mime_type`.
fn mime_to_pasteboard_type(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("public.png"),
        "image/jpeg" | "image/jpg" => Some("public.jpeg"),
        "image/gif" => Some("public.gif"),
        "image/webp" => Some("public.webp"),
        "image/svg+xml" => Some("public.svg-image"),
        _ => None,
    }
}

/// NSPasteboard UTI → MIME for image reads.
fn pasteboard_type_to_mime(uti: &str) -> Option<&'static str> {
    match uti {
        "public.png" => Some("image/png"),
        "public.jpeg" => Some("image/jpeg"),
        "public.gif" | "com.compuserve.gif" => Some("image/gif"),
        "public.webp" => Some("image/webp"),
        "public.svg-image" => Some("image/svg+xml"),
        _ => None,
    }
}

/// Image UTIs Warp's reader tries, in order of web-compatibility
/// preference. Matches
/// `crates/warpui/src/platform/mac/clipboard.rs::read_image_data_from_pasteboard`.
const READABLE_IMAGE_UTIS: [&str; 6] = [
    "public.png",
    "public.jpeg",
    "public.gif",
    "public.webp",
    "public.svg-image",
    "com.compuserve.gif",
];

fn read_images(pasteboard: &NSPasteboard) -> Option<Vec<ImageData>> {
    let mut images = Vec::new();
    for uti in READABLE_IMAGE_UTIS {
        let pb_type = NSString::from_str(uti);
        let data = unsafe { pasteboard.dataForType(&pb_type) };
        let Some(data) = data else { continue };
        if data.len() == 0 {
            continue;
        }
        let Some(mime) = pasteboard_type_to_mime(uti) else {
            continue;
        };
        images.push(ImageData {
            data: data.bytes().to_vec(),
            mime_type: mime.to_string(),
            // Warp also tries to lift a filename out of the HTML
            // portion of the clipboard (their `clipboard_utils`).
            // We don't do HTML parsing here — file metadata is a
            // polish layer we can add later when there's a consumer.
            filename: None,
        });
    }
    if images.is_empty() {
        None
    } else {
        Some(images)
    }
}

fn read_file_paths(pasteboard: &NSPasteboard) -> Option<Vec<String>> {
    // `readObjectsForClasses:options:` expects an NSArray of
    // `Class` instances, not protocol objects. The pattern is in
    // `objc2-app-kit-0.2.2/examples/nspasteboard.rs`: cast the
    // class pointer to `*mut AnyObject` and stash it inside an
    // `NSArray` for the call.
    let url_class: *const AnyClass = NSURL::class();
    let url_class = url_class as *mut AnyObject;
    let url_class = unsafe { Retained::retain(url_class) }?;
    let class_array = NSArray::from_vec(vec![url_class]);
    let objects = unsafe { pasteboard.readObjectsForClasses_options(&class_array, None) }?;

    let mut paths = Vec::with_capacity(objects.len());
    for i in 0..objects.len() {
        let any = unsafe { objects.objectAtIndex(i) };
        // Each entry was published as an NSURL because that's what
        // we asked for in `class_array`. Cast the AnyObject handle
        // to an NSURL pointer and re-retain.
        let ptr: *const AnyObject = Retained::as_ptr(&any);
        let url_ptr = ptr as *const NSURL as *mut NSURL;
        let Some(url) = (unsafe { Retained::retain(url_ptr) }) else {
            continue;
        };
        if let Some(path) = unsafe { url.path() } {
            paths.push(path.to_string());
        }
    }
    if paths.is_empty() {
        None
    } else {
        Some(paths)
    }
}
