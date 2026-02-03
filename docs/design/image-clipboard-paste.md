# Design: Image and File Clipboard Paste Support

## Problem Statement

Clipboard paste works for text but NOT for images, files, or other binary content. Users cannot paste screenshots or file references into Claude Code running inside the wrapper.

**Works:**
- Copy/paste text with Ctrl+C/Ctrl+V (via terminal's bracketed paste)

**Does Not Work:**
- Paste images (screenshots, copied images)
- Paste files or file paths
- Paste rich content

## Root Cause Analysis

### Why Text Paste Works

1. User presses Ctrl+V
2. Terminal emulator intercepts and triggers paste
3. Terminal sends bracketed paste sequence with text content
4. Crossterm receives `Event::Paste(String)`
5. Wrapper sends text to PTY with bracketed paste markers

### Why Image Paste Fails

1. User copies image to clipboard
2. User presses Ctrl+V
3. Terminal emulator cannot represent image as text
4. No `Event::Paste` event is generated (or empty string)
5. Nothing happens - image is silently ignored

**Key limitation:** Terminal events (`Event::Paste`) only support text. Terminals have no standard protocol for binary/image paste. The clipboard contains the image, but the terminal cannot communicate it.

## Solution Design

### Approach: Direct Clipboard Access + New Hotkey

Add direct system clipboard access using the `arboard` crate, which supports reading image and file data cross-platform. Provide a dedicated hotkey for paste operations that may include non-text content.

### Part 1: Add arboard Dependency

**File: `Cargo.toml`**

```toml
[dependencies]
arboard = { version = "3.6", default-features = true }  # image-data feature is default
```

### Part 2: Create Clipboard Module

**File: `src/clipboard.rs`**

New module to abstract clipboard operations:

```rust
use arboard::Clipboard;
use std::io::Write;
use std::path::PathBuf;

pub enum ClipboardContent {
    Text(String),
    Image(PathBuf),  // Path to saved temp file
    Files(Vec<PathBuf>),
    Empty,
}

pub struct ClipboardHandler {
    clipboard: Clipboard,
    temp_dir: PathBuf,
}

impl ClipboardHandler {
    pub fn new() -> Result<Self, arboard::Error> {
        let clipboard = Clipboard::new()?;
        let temp_dir = std::env::temp_dir().join("anyclaude");
        std::fs::create_dir_all(&temp_dir).ok();
        Ok(Self { clipboard, temp_dir })
    }

    /// Get clipboard content, preferring image over text
    pub fn get_content(&mut self) -> ClipboardContent {
        // Try image first
        if let Ok(image) = self.clipboard.get_image() {
            if let Some(path) = self.save_image(&image) {
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

    /// Get text content only (for normal paste)
    pub fn get_text(&mut self) -> Option<String> {
        self.clipboard.get_text().ok().filter(|t| !t.trim().is_empty())
    }

    fn save_image(&self, image: &arboard::ImageData) -> Option<PathBuf> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis();
        let filename = format!("paste_{}.png", timestamp);
        let path = self.temp_dir.join(&filename);

        // Convert RGBA to PNG and save
        let img = image::RgbaImage::from_raw(
            image.width as u32,
            image.height as u32,
            image.bytes.to_vec(),
        )?;
        img.save(&path).ok()?;

        Some(path)
    }
}
```

**Dependencies note:** Need to add `image` crate for PNG encoding:
```toml
image = { version = "0.25", default-features = false, features = ["png"] }
```

### Part 3: Add AppEvent for Image Paste

**File: `src/ui/events.rs`**

Add new event variant:

```rust
pub enum AppEvent {
    Input(KeyEvent),
    Paste(String),
    ImagePaste(PathBuf),  // NEW: path to pasted image file
    Tick,
    Resize(u16, u16),
    // ... existing variants
}
```

### Part 4: Add Wrapper Hotkey for Image Paste

**File: `src/ui/input.rs`**

Add Ctrl+Shift+V as dedicated "paste with image support" hotkey:

```rust
// Check for Ctrl+Shift+V (image paste hotkey)
if is_ctrl_shift_char(key, 'v') {
    // Request image paste - handled in runtime.rs
    return Some(InputAction::ImagePaste);
}

fn is_ctrl_shift_char(key: KeyEvent, needle: char) -> bool {
    matches!(key.code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&needle))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.modifiers.contains(KeyModifiers::SHIFT)
}
```

**Alternative:** If Ctrl+Shift+V is not reliably detected in terminal mode, use a different hotkey like Ctrl+Alt+V or add to wrapper's hotkey menu (Ctrl+B opens menu, then 'p' for paste).

### Part 5: Handle Image Paste in Runtime

**File: `src/ui/runtime.rs`**

When image paste is triggered:

```rust
InputAction::ImagePaste => {
    if let Ok(mut clipboard) = ClipboardHandler::new() {
        match clipboard.get_content() {
            ClipboardContent::Image(path) => {
                // Send file path as text input
                // Claude Code will read the image when referenced
                let path_str = path.to_string_lossy();
                let input = format!("{}\n", path_str);
                if let Some(pty) = app.pty() {
                    let _ = pty.send_input(input.as_bytes());
                }
            }
            ClipboardContent::Text(text) => {
                // Fall back to text paste
                app.on_paste(&text);
            }
            ClipboardContent::Files(paths) => {
                // Send file paths as text
                let paths_text = paths.iter()
                    .map(|p| p.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ");
                if let Some(pty) = app.pty() {
                    let bracketed = format!("\x1b[200~{}\x1b[201~", paths_text);
                    let _ = pty.send_input(bracketed.as_bytes());
                }
            }
            ClipboardContent::Empty => {}
        }
    }
}
```

### Part 6: Enhance Normal Paste Fallback

**File: `src/ui/app.rs`**

When normal `Event::Paste(text)` arrives with empty text, check for image:

```rust
pub fn on_paste(&mut self, text: &str, clipboard: &mut Option<ClipboardHandler>) {
    // If paste is empty, check for image content
    if text.trim().is_empty() {
        if let Some(clip) = clipboard {
            if let ClipboardContent::Image(path) = clip.get_content() {
                // Send image path
                if let Some(pty) = &self.pty {
                    let input = format!("{}\n", path.to_string_lossy());
                    let _ = pty.send_input(input.as_bytes());
                }
                return;
            }
        }
    }

    // Normal text paste
    let Some(pty) = &self.pty else { return; };
    let bracketed = format!("\x1b[200~{}\x1b[201~", text);
    let _ = pty.send_input(bracketed.as_bytes());
}
```

## Architecture Flow

```
┌──────────────────────────────────────────────────────────────┐
│ User Action                                                  │
├──────────────────────────────────────────────────────────────┤
│ Copy image → Ctrl+V  OR  Ctrl+Shift+V                        │
└───────────────────────────┬──────────────────────────────────┘
                            │
          ┌─────────────────┴─────────────────┐
          │                                   │
          ▼                                   ▼
┌─────────────────────┐             ┌─────────────────────────┐
│ Normal Paste        │             │ Image Paste Hotkey      │
│ (Ctrl+V)           │             │ (Ctrl+Shift+V)          │
└──────────┬──────────┘             └────────────┬────────────┘
           │                                     │
           ▼                                     │
┌─────────────────────┐                          │
│ Terminal handles    │                          │
│ Event::Paste(text) │                          │
└──────────┬──────────┘                          │
           │                                     │
           ▼                                     ▼
┌─────────────────────┐             ┌─────────────────────────┐
│ If text empty:      │────────────→│ ClipboardHandler        │
│ check clipboard     │             │ (arboard)               │
└──────────┬──────────┘             └────────────┬────────────┘
           │                                     │
           ▼                                     ▼
┌─────────────────────┐             ┌─────────────────────────┐
│ Send text to PTY    │             │ Detect content type     │
│ (bracketed paste)   │             │ Image? Text? Files?     │
└─────────────────────┘             └────────────┬────────────┘
                                                 │
                      ┌──────────────────────────┼───────────────┐
                      │                          │               │
                      ▼                          ▼               ▼
            ┌─────────────────┐      ┌───────────────┐  ┌──────────────┐
            │ Image:          │      │ Text:         │  │ Files:       │
            │ Save to temp    │      │ Bracketed     │  │ Send paths   │
            │ Send path       │      │ paste         │  │ as text      │
            └────────┬────────┘      └───────┬───────┘  └──────┬───────┘
                     │                       │                 │
                     └───────────────────────┴─────────────────┘
                                      │
                                      ▼
                           ┌─────────────────────┐
                           │ PTY → Claude Code   │
                           └─────────────────────┘
```

## File Changes Summary

| File | Change |
|------|--------|
| `Cargo.toml` | Add `arboard` and `image` dependencies |
| `src/clipboard.rs` | NEW: Clipboard access module |
| `src/lib.rs` | Add `mod clipboard` |
| `src/ui/events.rs` | Add `ImagePaste(PathBuf)` event variant |
| `src/ui/input.rs` | Add Ctrl+Shift+V handling |
| `src/ui/runtime.rs` | Handle image paste, init clipboard |
| `src/ui/app.rs` | Enhance on_paste with fallback to clipboard |

## Testing Plan

1. **Text paste (existing)**: Copy text → Ctrl+V → text appears in Claude Code
2. **Image paste via hotkey**: Copy screenshot → Ctrl+Shift+V → file path appears
3. **Image paste fallback**: Copy image → Ctrl+V → if empty paste, file path appears
4. **File paste**: Copy file in Finder → Ctrl+Shift+V → file path appears
5. **Temp file cleanup**: Verify temp files are created in expected location
6. **Cross-platform**: Test on macOS, Linux (X11/Wayland), Windows

## Platform Considerations

### macOS
- arboard uses `NSPasteboard` - well supported
- Images from screenshots work directly

### Linux
- X11: arboard uses `x11-clipboard` crate
- Wayland: Requires `wl-clipboard-rs` feature (optional)
- May need additional system dependencies

### Windows
- arboard uses Win32 clipboard API
- Well supported

## Dependencies

```toml
# Add to Cargo.toml
arboard = { version = "3.6", default-features = true }
image = { version = "0.25", default-features = false, features = ["png"] }
```

## Future Enhancements

1. **Temp file cleanup**: Add cleanup of old paste files on startup
2. **Drag and drop**: Handle file drag-drop events (if terminal supports)
3. **Rich paste menu**: Show paste options in UI when multiple formats available
4. **Base64 inline**: Option to paste image as base64 data URI instead of file path
