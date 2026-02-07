//! Clipboard access for image and file paste support.
//!
//! Provides direct system clipboard access using arboard, supporting:
//! - Text content
//! - Image content (saved to temp files as PNG)
//! - File paths (platform-dependent)

use arboard::Clipboard;
use base64::Engine as _;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};

/// Content types that can be read from the clipboard.
#[derive(Debug)]
pub enum ClipboardContent {
    /// Text content ready for paste.
    Text(String),
    /// Image encoded as data URI (PNG base64).
    Image(String),
    /// No usable content in clipboard.
    Empty,
}

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

    /// Get clipboard content, preferring image over text.
    ///
    /// If clipboard contains an image, encodes it as a data URI and returns it.
    /// Otherwise returns text content if available.
    pub fn get_content(&mut self) -> ClipboardContent {
        // Try image first
        if let Ok(image_data) = self.clipboard.get_image() {
            if let Some(data_uri) = self.image_to_data_uri(&image_data) {
                return ClipboardContent::Image(data_uri);
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

    /// Check if clipboard has image content without consuming it.
    pub fn has_image(&mut self) -> bool {
        self.clipboard.get_image().is_ok()
    }

    /// Encode image data as a PNG or JPEG data URI.
    fn image_to_data_uri(&self, image_data: &arboard::ImageData) -> Option<String> {
        let width = image_data.width as u32;
        let height = image_data.height as u32;
        let has_alpha = image_data.bytes.chunks(4).any(|pixel| pixel[3] != 255);

        if has_alpha {
            let mut png_bytes = Vec::new();
            let encoder = PngEncoder::new(&mut png_bytes);
            encoder
                .write_image(&image_data.bytes, width, height, ExtendedColorType::Rgba8)
                .ok()?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
            Some(format!("data:image/png;base64,{}", b64))
        } else {
            let mut rgb_bytes = Vec::with_capacity((width * height * 3) as usize);
            for pixel in image_data.bytes.chunks(4) {
                rgb_bytes.push(pixel[0]);
                rgb_bytes.push(pixel[1]);
                rgb_bytes.push(pixel[2]);
            }
            let mut jpeg_bytes = Vec::new();
            let encoder = JpegEncoder::new_with_quality(&mut jpeg_bytes, 85);
            encoder
                .write_image(&rgb_bytes, width, height, ExtendedColorType::Rgb8)
                .ok()?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);
            Some(format!("data:image/jpeg;base64,{}", b64))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_handler_creation() {
        // This may fail in CI without display, so just check it compiles
        let _handler = ClipboardHandler::new();
    }
}
