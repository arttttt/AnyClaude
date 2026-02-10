//! Clipboard access for image and file paste support.
//!
//! Provides direct system clipboard access using arboard, supporting:
//! - Text content
//! - Image content (saved to temp files as PNG/JPEG)

use arboard::Clipboard;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use std::io::Write;
use std::path::PathBuf;
use tempfile::NamedTempFile;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Maximum raw image size in bytes (50 MB RGBA).
const MAX_IMAGE_SIZE: usize = 50 * 1024 * 1024;

/// Content types that can be read from the clipboard.
#[derive(Debug)]
pub enum ClipboardContent {
    /// Text content ready for paste.
    Text(String),
    /// Image saved to a temp file.
    Image(PathBuf),
    /// No usable content in clipboard.
    Empty,
}

/// Handler for clipboard operations.
pub struct ClipboardHandler {
    clipboard: Clipboard,
    temp_dir: PathBuf,
    last_error: Option<String>,
}

impl ClipboardHandler {
    /// Create a new clipboard handler.
    pub fn new() -> Result<Self, arboard::Error> {
        let clipboard = Clipboard::new()?;
        let temp_dir = std::env::temp_dir().join("anyclaude");
        std::fs::create_dir_all(&temp_dir).ok();
        Ok(Self {
            clipboard,
            temp_dir,
            last_error: None,
        })
    }

    /// Take the last error, if any, clearing it.
    pub fn take_error(&mut self) -> Option<String> {
        self.last_error.take()
    }

    /// Write text to the system clipboard.
    pub fn set_text(&mut self, text: &str) -> Result<(), String> {
        self.clipboard
            .set_text(text.to_string())
            .map_err(|e| format!("Failed to set clipboard text: {}", e))
    }

    /// Get clipboard content, preferring image over text.
    ///
    /// If clipboard contains an image, saves it to a temp file and returns the path.
    /// Otherwise returns text content if available.
    /// On failure, the error is stored and can be retrieved via `take_error()`.
    pub fn get_content(&mut self) -> ClipboardContent {
        self.last_error = None;

        // Try image first
        if let Ok(image_data) = self.clipboard.get_image() {
            match self.save_image(&image_data) {
                Ok(path) => return ClipboardContent::Image(path),
                Err(err) => self.last_error = Some(err),
            }
        }

        // Fall back to text
        if let Ok(text) = self.clipboard.get_text() {
            if !text.trim().is_empty() {
                return ClipboardContent::Text(text);
            }
        }

        ClipboardContent::Empty
    }

    /// Save image data to a temp file as PNG (with alpha) or JPEG (opaque).
    fn save_image(&self, image_data: &arboard::ImageData) -> Result<PathBuf, String> {
        let estimated_size = image_data.width * image_data.height * 4;
        if estimated_size > MAX_IMAGE_SIZE {
            return Err(format!(
                "Image too large: {} bytes (max: {})",
                estimated_size, MAX_IMAGE_SIZE
            ));
        }

        let width = image_data.width as u32;
        let height = image_data.height as u32;
        let has_alpha = image_data.bytes.chunks(4).any(|pixel| pixel[3] != 255);

        let encoded = if has_alpha {
            let mut buf = Vec::new();
            PngEncoder::new(&mut buf)
                .write_image(&image_data.bytes, width, height, ExtendedColorType::Rgba8)
                .map_err(|e| format!("Failed to encode PNG ({}x{}): {}", width, height, e))?;
            buf
        } else {
            let mut rgb = Vec::with_capacity((width * height * 3) as usize);
            for pixel in image_data.bytes.chunks(4) {
                rgb.extend_from_slice(&pixel[..3]);
            }
            let mut buf = Vec::new();
            JpegEncoder::new_with_quality(&mut buf, 85)
                .write_image(&rgb, width, height, ExtendedColorType::Rgb8)
                .map_err(|e| format!("Failed to encode JPEG ({}x{}): {}", width, height, e))?;
            buf
        };

        write_temp_file(&self.temp_dir, &encoded)
    }
}

/// Write bytes to a new temp file with restricted permissions.
fn write_temp_file(dir: &PathBuf, data: &[u8]) -> Result<PathBuf, String> {
    let mut file = NamedTempFile::new_in(dir)
        .map_err(|e| format!("Failed to create temp file: {}", e))?;

    file.write_all(data)
        .map_err(|e| format!("Failed to write temp file ({} bytes): {}", data.len(), e))?;

    #[cfg(unix)]
    {
        let perms = std::fs::Permissions::from_mode(0o600);
        file.as_file()
            .set_permissions(perms)
            .map_err(|e| format!("Failed to set temp file permissions: {}", e))?;
    }

    let (_, path) = file
        .keep()
        .map_err(|e| format!("Failed to persist temp file: {}", e))?;
    Ok(path)
}
