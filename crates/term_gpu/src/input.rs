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

/// Encode a mouse event in the legacy X10 form `CSI M Cb Cx Cy`, each value a
/// single byte offset by 32. `button` is the raw button-bits value (0 = left,
/// 1 = middle, 2 = right, 3 = release; 64 / 65 = wheel up / down). `col` / `row`
/// are 1-based cells; values above 223 can't fit a single byte and are clamped
/// (the SGR form has no such limit).
pub fn encode_mouse_x10(button: u8, col: u16, row: u16) -> Vec<u8> {
    let enc = |v: u16| 32u8.saturating_add(v.min(223) as u8);
    vec![0x1b, b'[', b'M', 32u8.saturating_add(button), enc(col), enc(row)]
}

/// Encode a mouse event in SGR form `CSI < Cb ; Cx ; Cy (M|m)` — `M` for a
/// press / wheel, `m` for a release. `button` is the raw button-bits value;
/// `col` / `row` are 1-based with no coordinate limit.
pub fn encode_mouse_sgr(button: u8, col: u16, row: u16, pressed: bool) -> Vec<u8> {
    let final_byte = if pressed { 'M' } else { 'm' };
    format!("\x1b[<{button};{col};{row}{final_byte}").into_bytes()
}

/// A mouse button in xterm-reporting terms (the events anyclaude forwards).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

impl MouseButton {
    /// The base button-bits value, before the motion adjustment.
    fn base(self) -> u8 {
        match self {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::WheelUp => 64,
            MouseButton::WheelDown => 65,
        }
    }
}

/// What happened to the button. `Motion` is a drag (a button held while the
/// pointer moves) or bare pointer motion under any-event tracking; it sets the
/// `+32` motion bit. xterm has no wheel release, so wheels are always `Press`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Press,
    Release,
    Motion,
}

/// Compose an xterm mouse report for `(button, kind)` at 1-based cell
/// `(col, row)`. `sgr` selects the SGR (1006) form over the legacy default
/// (`CSI M`) form. This is the one place the protocol byte shape is decided —
/// the coordinator maps platform events to `(MouseButton, MouseEventKind)` and
/// reads the encoding off the emulator's [`term_core::MouseProtocol`]. Modifier
/// keys are intentionally not folded into `Cb` (matching Warp); the UTF-8 (1005)
/// and urxvt (1015) encodings are intentionally unsupported (deprecated, and
/// Warp omits them too).
pub fn encode_mouse_report(
    button: MouseButton,
    kind: MouseEventKind,
    col: u16,
    row: u16,
    sgr: bool,
) -> Vec<u8> {
    let motion = matches!(kind, MouseEventKind::Motion);
    let cb = button.base() + if motion { 32 } else { 0 };
    if sgr {
        // SGR keeps the real button code; press / motion → 'M', release → 'm'.
        encode_mouse_sgr(cb, col, row, !matches!(kind, MouseEventKind::Release))
    } else {
        // Legacy form carries no button identity on release — it is reported as
        // button-bits 3.
        let raw = if matches!(kind, MouseEventKind::Release) { 3 } else { cb };
        encode_mouse_x10(raw, col, row)
    }
}
