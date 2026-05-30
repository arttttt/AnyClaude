//! VT-style keyboard encoding for winit key events.
//!
//! Maps `winit::keyboard::Key` + `ModifiersState` to the byte sequence
//! a typical terminal sends to the PTY. Covers printable text, named
//! keys (Enter / Tab / arrows / home-end / page up-down / delete),
//! `Ctrl+letter` control codes, and `Alt+key` as ESC-prefixed Meta.
//! Returns `None` for keys that have no terminal-byte equivalent
//! (modifier keys alone, function keys we don't translate, IME
//! composition events).

use term_core::{MouseProtocol, MouseTracking};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// The xterm modifier parameter (`1 + shift + alt*2 + ctrl*4`) as its ASCII
/// digit (`'2'..'8'`), or `None` when no shift / alt / ctrl is held (the
/// unmodified form). Cmd / Super is never encoded — it drives app shortcuts.
fn modifier_param(m: ModifiersState) -> Option<u8> {
    let bits =
        (m.shift_key() as u8) | ((m.alt_key() as u8) << 1) | ((m.control_key() as u8) << 2);
    (bits != 0).then_some(b'1' + bits)
}

/// Arrow / Home / End: `CSI 1 ; <mod> <letter>` when a modifier is held;
/// otherwise the application-cursor `SS3 <letter>` (DECCKM) or the normal
/// `CSI <letter>` form. Modified forms always use `CSI`, even under DECCKM.
fn cursor_seq(letter: u8, m: ModifiersState, app_cursor: bool) -> Vec<u8> {
    match modifier_param(m) {
        Some(p) => vec![0x1b, b'[', b'1', b';', p, letter],
        None if app_cursor => vec![0x1b, b'O', letter],
        None => vec![0x1b, b'[', letter],
    }
}

/// F1–F4: `SS3 P/Q/R/S`, or `CSI 1 ; <mod> P/Q/R/S` when modified.
fn fn_pqrs(letter: u8, m: ModifiersState) -> Vec<u8> {
    match modifier_param(m) {
        Some(p) => vec![0x1b, b'[', b'1', b';', p, letter],
        None => vec![0x1b, b'O', letter],
    }
}

/// F5+ : `CSI <n> ~`, or `CSI <n> ; <mod> ~` when modified.
fn fn_tilde(n: &[u8], m: ModifiersState) -> Vec<u8> {
    let mut v = vec![0x1b, b'['];
    v.extend_from_slice(n);
    if let Some(p) = modifier_param(m) {
        v.push(b';');
        v.push(p);
    }
    v.push(b'~');
    v
}

/// Encode a key press as the PTY input bytes. `key` is the layout-resolved
/// logical key (modifier-composed — e.g. macOS `Option+a` arrives as `å`);
/// `key_unmod` is the same key WITHOUT modifiers (the base `a`), used for the
/// Meta / ESC-prefix form so `Option+a` sends `ESC a`, not `ESC å`.
/// `app_cursor` is the emulator's DECCKM state (arrows/Home/End use `SS3` when
/// set). Returns `None` when the key has no terminal-byte equivalent. Cmd /
/// Super combos are handled upstream as app shortcuts, never here.
pub fn encode_key(
    key: &Key,
    key_unmod: &Key,
    modifiers: ModifiersState,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    let ctrl = modifiers.control_key();
    let alt = modifiers.alt_key();
    match key {
        Key::Character(s) => {
            let chars: Vec<char> = s.chars().collect();
            if ctrl && chars.len() == 1 {
                let ch = chars[0];
                if ch.is_ascii_alphabetic() {
                    // Ctrl+A..Z → 0x01..0x1A; Ctrl+Alt+key adds the ESC prefix.
                    let c0 = ch.to_ascii_lowercase() as u8 - b'a' + 1;
                    return Some(if alt { vec![0x1b, c0] } else { vec![c0] });
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
            if alt {
                // Meta: ESC + the un-composed base char (so macOS Option+a is
                // `ESC a`, not the composed `å`).
                if let Key::Character(base) = key_unmod {
                    let mut bytes = vec![0x1b];
                    bytes.extend_from_slice(base.as_str().as_bytes());
                    return Some(bytes);
                }
            }
            Some(s.as_str().as_bytes().to_vec())
        }
        Key::Named(named) => match named {
            NamedKey::Enter => Some(b"\r".to_vec()),
            // Shift+Tab is back-tab (CSI Z) — ink TUIs use it for mode cycling.
            NamedKey::Tab if modifiers.shift_key() => Some(b"\x1b[Z".to_vec()),
            NamedKey::Tab => Some(b"\t".to_vec()),
            NamedKey::Backspace => Some(b"\x7f".to_vec()),
            NamedKey::Escape => Some(b"\x1b".to_vec()),
            NamedKey::Space => Some(b" ".to_vec()),
            NamedKey::ArrowUp => Some(cursor_seq(b'A', modifiers, app_cursor)),
            NamedKey::ArrowDown => Some(cursor_seq(b'B', modifiers, app_cursor)),
            NamedKey::ArrowRight => Some(cursor_seq(b'C', modifiers, app_cursor)),
            NamedKey::ArrowLeft => Some(cursor_seq(b'D', modifiers, app_cursor)),
            NamedKey::Home => Some(cursor_seq(b'H', modifiers, app_cursor)),
            NamedKey::End => Some(cursor_seq(b'F', modifiers, app_cursor)),
            NamedKey::Insert => Some(b"\x1b[2~".to_vec()),
            NamedKey::Delete => Some(b"\x1b[3~".to_vec()),
            NamedKey::PageUp => Some(b"\x1b[5~".to_vec()),
            NamedKey::PageDown => Some(b"\x1b[6~".to_vec()),
            NamedKey::F1 => Some(fn_pqrs(b'P', modifiers)),
            NamedKey::F2 => Some(fn_pqrs(b'Q', modifiers)),
            NamedKey::F3 => Some(fn_pqrs(b'R', modifiers)),
            NamedKey::F4 => Some(fn_pqrs(b'S', modifiers)),
            NamedKey::F5 => Some(fn_tilde(b"15", modifiers)),
            NamedKey::F6 => Some(fn_tilde(b"17", modifiers)),
            NamedKey::F7 => Some(fn_tilde(b"18", modifiers)),
            NamedKey::F8 => Some(fn_tilde(b"19", modifiers)),
            NamedKey::F9 => Some(fn_tilde(b"20", modifiers)),
            NamedKey::F10 => Some(fn_tilde(b"21", modifiers)),
            NamedKey::F11 => Some(fn_tilde(b"23", modifiers)),
            NamedKey::F12 => Some(fn_tilde(b"24", modifiers)),
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
    /// No button — the "no buttons pressed" code (3), used for bare any-event
    /// (1003) pointer motion.
    None,
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
            MouseButton::None => 3,
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

/// Decide the motion (drag / move) report when the pointer moves to 0-based
/// `cell`, for a mouse-tracking app. Returns `None` when motion isn't reported:
/// off / click-only (1000) tracking, button-event (1002) tracking with no
/// button held, or the pointer is still in `last_cell` (report once per cell
/// crossed, not once per pixel). Under any-event (1003) tracking a held button
/// reports a drag, otherwise a bare `None`-button move. The caller owns the
/// Shift bypass and updating `last_cell` once a report is produced.
pub fn encode_motion_report(
    proto: MouseProtocol,
    left_held: bool,
    last_cell: Option<(u16, u16)>,
    cell: (u16, u16),
) -> Option<Vec<u8>> {
    if last_cell == Some(cell) {
        return None;
    }
    let button = match proto.tracking {
        MouseTracking::ButtonEvent if left_held => MouseButton::Left,
        MouseTracking::AnyEvent => {
            if left_held {
                MouseButton::Left
            } else {
                MouseButton::None
            }
        }
        _ => return None,
    };
    Some(encode_mouse_report(
        button,
        MouseEventKind::Motion,
        cell.0 + 1,
        cell.1 + 1,
        proto.is_sgr(),
    ))
}
