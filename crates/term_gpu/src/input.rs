//! VT-style keyboard encoding for winit key events.
//!
//! Maps `winit::keyboard::Key` + `ModifiersState` to the byte sequence
//! a typical terminal sends to the PTY. Covers printable text, named
//! keys (Enter / Tab / arrows / home-end / page up-down / delete),
//! `Ctrl+letter` control codes, and `Alt+key` as ESC-prefixed Meta.
//! Returns `None` for keys that have no terminal-byte equivalent
//! (modifier keys alone, function keys we don't translate, IME
//! composition events).

use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Encode `(key, modifiers)` as PTY input bytes. Returns `None` when
/// the key has no terminal-byte equivalent.
pub fn encode_key(key: &Key, modifiers: ModifiersState) -> Option<Vec<u8>> {
    let ctrl = modifiers.control_key();
    let alt = modifiers.alt_key();
    match key {
        Key::Character(s) => {
            let chars: Vec<char> = s.chars().collect();
            if ctrl && chars.len() == 1 {
                let ch = chars[0];
                if ch.is_ascii_alphabetic() {
                    // Ctrl+A..Z → 0x01..0x1A.
                    return Some(vec![(ch.to_ascii_lowercase() as u8) - b'a' + 1]);
                }
                // A few non-letter Ctrl combos shells expect to receive.
                let mapped = match ch {
                    '[' => Some(0x1b),
                    '\\' => Some(0x1c),
                    ']' => Some(0x1d),
                    '~' | '^' => Some(0x1e),
                    '?' | '/' => Some(0x1f),
                    ' ' => Some(0x00),
                    _ => None,
                };
                if let Some(b) = mapped {
                    return Some(vec![b]);
                }
            }
            let mut bytes = s.as_str().as_bytes().to_vec();
            if alt {
                // ESC-prefix is the conventional encoding for Meta+key.
                bytes.insert(0, 0x1b);
            }
            Some(bytes)
        }
        Key::Named(named) => match named {
            NamedKey::Enter => Some(b"\r".to_vec()),
            NamedKey::Tab => Some(b"\t".to_vec()),
            NamedKey::Backspace => Some(b"\x7f".to_vec()),
            NamedKey::Escape => Some(b"\x1b".to_vec()),
            NamedKey::Space => Some(b" ".to_vec()),
            NamedKey::ArrowUp => Some(b"\x1b[A".to_vec()),
            NamedKey::ArrowDown => Some(b"\x1b[B".to_vec()),
            NamedKey::ArrowRight => Some(b"\x1b[C".to_vec()),
            NamedKey::ArrowLeft => Some(b"\x1b[D".to_vec()),
            NamedKey::Home => Some(b"\x1b[H".to_vec()),
            NamedKey::End => Some(b"\x1b[F".to_vec()),
            NamedKey::Delete => Some(b"\x1b[3~".to_vec()),
            NamedKey::PageUp => Some(b"\x1b[5~".to_vec()),
            NamedKey::PageDown => Some(b"\x1b[6~".to_vec()),
            _ => None,
        },
        _ => None,
    }
}
