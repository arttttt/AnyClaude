//! Cross-platform clipboard primitives for the AnyClaude GPU terminal.
//!
//! Modeled on Warp's `warpui_core::clipboard` (MIT, see
//! `crates/warpui_core/src/clipboard.rs` in warpdotdev/warp). A
//! [`Clipboard`] hides the OS pasteboard behind read/write of a
//! [`ClipboardContent`] bundle that can carry plain text, file paths,
//! HTML, and bitmap images.
//!
//! macOS uses [`MacClipboard`] via NSPasteboard. Other platforms
//! currently have no native implementation — fall back to
//! [`InMemoryClipboard`].

use std::sync::Mutex;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::MacClipboard;

/// Read and write the system clipboard.
///
/// Implementations must be `Send + 'static` so a `Box<dyn Clipboard>`
/// can live on app state and survive across threads (e.g. PTY readers
/// invoking a paste through a proxy channel).
pub trait Clipboard: Send + 'static {
    /// Replace the clipboard's contents with `contents`.
    fn write(&mut self, contents: ClipboardContent);

    /// Read whatever's currently on the clipboard.
    fn read(&mut self) -> ClipboardContent;

    /// Write to the primary clipboard (X11 / Wayland middle-click).
    /// On platforms without a primary clipboard, falls back to
    /// [`Clipboard::write`].
    fn write_to_primary_clipboard(&mut self, contents: ClipboardContent) {
        self.write(contents);
    }

    /// Read from the primary clipboard (X11 / Wayland middle-click).
    /// On platforms without a primary clipboard, falls back to
    /// [`Clipboard::read`].
    fn read_from_primary_clipboard(&mut self) -> ClipboardContent {
        self.read()
    }
}

/// A clipboard payload. Any combination of these fields may be set
/// when reading; writers typically populate just `plain_text`.
#[derive(Debug, Clone, Default)]
pub struct ClipboardContent {
    /// UTF-8 plain text. Empty string when absent.
    pub plain_text: String,
    /// File paths (e.g. dropped from Finder / Files). `None` when none
    /// were on the clipboard.
    pub paths: Option<Vec<String>>,
    /// HTML markup, if the source app published HTML alongside plain
    /// text.
    pub html: Option<String>,
    /// Bitmap images, in order of publisher preference.
    pub images: Option<Vec<ImageData>>,
}

/// One image attached to a [`ClipboardContent`].
#[derive(Debug, Clone)]
pub struct ImageData {
    /// Raw image bytes as published by the source (PNG / JPEG / etc.).
    pub data: Vec<u8>,
    /// MIME type, e.g. `"image/png"`.
    pub mime_type: String,
    /// Source filename, if known. Often `None` — the OS pasteboard
    /// doesn't always carry one.
    pub filename: Option<String>,
}

impl ClipboardContent {
    /// Shortcut for plain-text-only content.
    pub fn plain_text(text: String) -> Self {
        Self {
            plain_text: text,
            paths: None,
            html: None,
            images: None,
        }
    }

    /// `true` when no field carries any value (no text, no paths, no
    /// HTML, no images).
    pub fn is_empty(&self) -> bool {
        let Self {
            plain_text,
            paths,
            html,
            images,
        } = self;
        plain_text.is_empty() && paths.is_none() && html.is_none() && images.is_none()
    }

    /// `true` when at least one image was present on the clipboard.
    pub fn has_image_data(&self) -> bool {
        self.images
            .as_ref()
            .map(|imgs| !imgs.is_empty())
            .unwrap_or(false)
    }

    /// Number of file paths in `paths` (0 if `None`).
    pub fn num_paths(&self) -> usize {
        self.paths.as_ref().map(|p| p.len()).unwrap_or(0)
    }

    /// `true` when at least one path does NOT look like an image
    /// (mixed content: some paths are documents / archives / etc.,
    /// not screenshots).
    pub fn has_non_image_filepaths(&self) -> bool {
        self.paths
            .as_ref()
            .map(|paths| paths.iter().any(|p| !has_image_extension(p)))
            .unwrap_or(false)
    }
}

/// Heuristic used by paste handlers to decide whether to also insert
/// the plain-text portion of a clipboard payload. Mirrors Warp's
/// `should_insert_text_on_paste`. The general rule:
///
/// 1. No images and no paths at all → insert text.
/// 2. Has non-image file paths (mixed content) → insert text (user
///    likely wants the path).
/// 3. Has image data but no file paths (direct image paste) → insert
///    text too if any is present (some apps attach a caption).
///
/// The negation handles "only image-paths, no text" — most terminals
/// would skip the paste in that case to avoid pasting an image path
/// they can't display.
pub fn should_insert_text_on_paste(content: &ClipboardContent) -> bool {
    if !content.has_image_data() && content.num_paths() == 0 {
        return true;
    }
    if content.has_non_image_filepaths() {
        return true;
    }
    content.has_image_data() && content.num_paths() == 0
}

/// File extensions recognised as "this path is an image". Matches
/// Warp's `crates/warpui_core/src/clipboard_utils.rs::IMAGE_EXTENSIONS`.
pub const IMAGE_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp"];

/// Preferred image MIME types for clipboard operations, in priority
/// order. Mirrors Warp's
/// `crates/warpui_core/src/clipboard_utils.rs::CLIPBOARD_IMAGE_MIME_TYPES`.
/// Consumers (e.g. paste handlers) walk this list to pick the best
/// available image MIME when several are on the clipboard.
pub const CLIPBOARD_IMAGE_MIME_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/jpg",
    "image/gif",
    "image/webp",
];

/// `true` when the path's lowercased trailing characters match a
/// known image extension. Used by [`ClipboardContent::has_non_image_filepaths`]
/// and exposed for downstream paste handlers that filter file paths.
pub fn has_image_extension(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    IMAGE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// Return only the file paths that look like image files. Matches
/// Warp's `get_image_filepaths_from_paths`.
pub fn get_image_filepaths_from_paths(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|p| has_image_extension(p))
        .cloned()
        .collect()
}

/// In-process clipboard. The content lives in a mutex and is scoped to
/// the running process — useful for tests and as a fallback on
/// platforms where the native clipboard backend is unavailable.
pub struct InMemoryClipboard {
    content: Mutex<ClipboardContent>,
    primary: Mutex<ClipboardContent>,
}

impl Default for InMemoryClipboard {
    fn default() -> Self {
        Self {
            content: Mutex::new(ClipboardContent::default()),
            primary: Mutex::new(ClipboardContent::default()),
        }
    }
}

impl Clipboard for InMemoryClipboard {
    fn write(&mut self, contents: ClipboardContent) {
        *self.content.lock().expect("clipboard mutex") = contents;
    }

    fn read(&mut self) -> ClipboardContent {
        self.content.lock().expect("clipboard mutex").clone()
    }

    fn write_to_primary_clipboard(&mut self, contents: ClipboardContent) {
        *self.primary.lock().expect("primary clipboard mutex") = contents;
    }

    fn read_from_primary_clipboard(&mut self) -> ClipboardContent {
        self.primary.lock().expect("primary clipboard mutex").clone()
    }
}
