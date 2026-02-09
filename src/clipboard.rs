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

/// Максимальный размер изображения в байтах (50MB)
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
        })
    }

    /// Get clipboard content, preferring image over text.
    ///
    /// If clipboard contains an image, saves it to a temp file and returns the path.
    /// Otherwise returns text content if available.
    pub fn get_content(&mut self) -> ClipboardContent {
        // Try image first
        if let Ok(image_data) = self.clipboard.get_image() {
            if let Some(path) = self.save_image(&image_data) {
                return ClipboardContent::Image(path);
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
    fn save_image(&self, image_data: &arboard::ImageData) -> Option<PathBuf> {
        // Check image size before encoding/saving
        let estimated_size = image_data.width * image_data.height * 4; // RGBA
        if estimated_size > MAX_IMAGE_SIZE {
            eprintln!("Image too large: {} bytes (max: {})", estimated_size, MAX_IMAGE_SIZE);
            return None;
        }

        let width = image_data.width as u32;
        let height = image_data.height as u32;
        let has_alpha = image_data.bytes.chunks(4).any(|pixel| pixel[3] != 255);

        if has_alpha {
            let mut bytes = Vec::new();
            let encoder = PngEncoder::new(&mut bytes);
            encoder
                .write_image(&image_data.bytes, width, height, ExtendedColorType::Rgba8)
                .map_err(|e| {
                    eprintln!("Failed to encode PNG image ({}x{}): {}", width, height, e);
                    e
                })
                .ok()?;

            let mut temp_file = NamedTempFile::new_in(&self.temp_dir)
                .map_err(|e| {
                    eprintln!("Failed to create temp file: {}", e);
                    e
                })
                .ok()?;

            temp_file.write_all(&bytes).map_err(|e| {
                eprintln!(
                    "Failed to write PNG data to temp file: {} (size: {} bytes)",
                    e,
                    bytes.len()
                );
                e
            }).ok()?;

            #[cfg(unix)]
            {
                let mut perms = temp_file.as_file().metadata().ok()?.permissions();
                perms.set_mode(0o600);
                temp_file.as_file().set_permissions(perms).ok()?;
            }

            let (_, path) = temp_file.keep().ok()?;
            Some(path)
        } else {
            let mut rgb_bytes = Vec::with_capacity((width * height * 3) as usize);
            for pixel in image_data.bytes.chunks(4) {
                rgb_bytes.push(pixel[0]);
                rgb_bytes.push(pixel[1]);
                rgb_bytes.push(pixel[2]);
            }
            let mut bytes = Vec::new();
            let encoder = JpegEncoder::new_with_quality(&mut bytes, 85);
            encoder
                .write_image(&rgb_bytes, width, height, ExtendedColorType::Rgb8)
                .map_err(|e| {
                    eprintln!("Failed to encode JPEG image ({}x{}): {}", width, height, e);
                    e
                })
                .ok()?;

            let mut temp_file = NamedTempFile::new_in(&self.temp_dir)
                .map_err(|e| {
                    eprintln!("Failed to create temp file: {}", e);
                    e
                })
                .ok()?;

            temp_file.write_all(&bytes).map_err(|e| {
                eprintln!(
                    "Failed to write JPEG data to temp file: {} (size: {} bytes)",
                    e,
                    bytes.len()
                );
                e
            }).ok()?;

            #[cfg(unix)]
            {
                let mut perms = temp_file.as_file().metadata().ok()?.permissions();
                perms.set_mode(0o600);
                temp_file.as_file().set_permissions(perms).ok()?;
            }

            let (_, path) = temp_file.keep().ok()?;
            Some(path)
        }
    }
}
