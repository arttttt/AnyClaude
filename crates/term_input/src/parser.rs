use crate::event::{
    Direction, InputEvent, KeyInput, KeyKind, MouseButton, MouseEvent, NavKey,
};
use std::collections::VecDeque;

/// Parser state.
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Ground,
    EscReceived,
    CsiEntry,
    CsiParam,
    Ss3Pending,
    MouseX10 { count: u8 },
    SgrMouse,
    PasteBuf,
    /// Collecting UTF-8 continuation bytes. `remaining` = bytes still needed.
    Utf8 { remaining: u8 },
}

/// Terminal input parser — pure state machine, no I/O.
///
/// Feed raw bytes via `feed()`, collect parsed `InputEvent`s.
/// Call `flush()` after a timeout to finalize pending ESC sequences.
pub struct InputParser {
    state: State,
    buf: Vec<u8>,
    paste_buf: String,
    out: VecDeque<InputEvent>,
}

impl InputParser {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            buf: Vec::with_capacity(32),
            paste_buf: String::new(),
            out: VecDeque::new(),
        }
    }

    /// Feed raw bytes to the parser. Returns all fully parsed events.
    pub fn feed(&mut self, data: &[u8]) -> Vec<InputEvent> {
        for &byte in data {
            self.step(byte);
        }
        self.out.drain(..).collect()
    }

    /// Flush pending state (ESC timeout). Call after a short timeout with no new data.
    pub fn flush(&mut self) -> Vec<InputEvent> {
        match self.state {
            State::EscReceived => {
                self.emit_key(vec![0x1b], KeyKind::Escape);
                self.state = State::Ground;
                self.buf.clear();
            }
            State::CsiEntry | State::CsiParam | State::Ss3Pending | State::SgrMouse => {
                // Incomplete sequence — emit as Unknown
                let raw = std::mem::take(&mut self.buf);
                self.emit_key(raw, KeyKind::Unknown);
                self.state = State::Ground;
            }
            State::MouseX10 { .. } | State::Utf8 { .. } => {
                let raw = std::mem::take(&mut self.buf);
                self.emit_key(raw, KeyKind::Unknown);
                self.state = State::Ground;
            }
            State::PasteBuf => {
                // Incomplete paste — emit what we have
                let text = std::mem::take(&mut self.paste_buf);
                self.out.push_back(InputEvent::Paste(text));
                self.buf.clear();
                self.state = State::Ground;
            }
            State::Ground => {}
        }
        self.out.drain(..).collect()
    }

    /// Whether there is pending state that needs a timeout to resolve.
    pub fn has_pending(&self) -> bool {
        self.state != State::Ground
    }

    fn step(&mut self, byte: u8) {
        match self.state {
            State::Ground => self.ground(byte),
            State::EscReceived => self.esc_received(byte),
            State::CsiEntry => self.csi_entry(byte),
            State::CsiParam => self.csi_param(byte),
            State::Ss3Pending => self.ss3_pending(byte),
            State::MouseX10 { count } => self.mouse_x10(byte, count),
            State::SgrMouse => self.sgr_mouse(byte),
            State::PasteBuf => self.paste_buf(byte),
            State::Utf8 { remaining } => self.utf8_cont(byte, remaining),
        }
    }

    fn ground(&mut self, byte: u8) {
        match byte {
            0x1b => {
                self.buf.clear();
                self.buf.push(0x1b);
                self.state = State::EscReceived;
            }
            0x0d => self.emit_key(vec![byte], KeyKind::Enter),
            0x09 => self.emit_key(vec![byte], KeyKind::Tab),
            0x7f => self.emit_key(vec![byte], KeyKind::Backspace),
            // Ctrl+A..Ctrl+Z (except Tab=0x09, Enter=0x0d, ESC=0x1b)
            0x01..=0x1a => {
                let letter = (b'a' + byte - 1) as char;
                self.emit_key(vec![byte], KeyKind::Control(letter));
            }
            // Printable ASCII
            0x20..=0x7e => {
                self.emit_key(vec![byte], KeyKind::Char(byte as char));
            }
            // UTF-8 lead bytes (remaining = total_bytes - 1 for the lead byte)
            0xc0..=0xdf => self.start_utf8(byte, 1),
            0xe0..=0xef => self.start_utf8(byte, 2),
            0xf0..=0xf7 => self.start_utf8(byte, 3),
            // Other bytes: emit as-is
            _ => self.emit_key(vec![byte], KeyKind::Unknown),
        }
    }

    fn start_utf8(&mut self, byte: u8, remaining: u8) {
        self.buf.clear();
        self.buf.push(byte);
        self.state = State::Utf8 { remaining };
    }

    fn utf8_cont(&mut self, byte: u8, remaining: u8) {
        if byte & 0xC0 != 0x80 {
            // Not a valid continuation byte — emit what we have as Unknown, reprocess this byte
            let raw = std::mem::take(&mut self.buf);
            self.emit_key(raw, KeyKind::Unknown);
            self.state = State::Ground;
            self.step(byte);
            return;
        }
        self.buf.push(byte);
        let new_remaining = remaining - 1;
        if new_remaining > 0 {
            self.state = State::Utf8 { remaining: new_remaining };
            return;
        }
        // All bytes collected — decode
        let raw = std::mem::take(&mut self.buf);
        let kind = match std::str::from_utf8(&raw) {
            Ok(s) => {
                let ch = s.chars().next().unwrap_or('\u{FFFD}');
                KeyKind::Char(ch)
            }
            Err(_) => KeyKind::Unknown,
        };
        self.emit_key(raw, kind);
        self.state = State::Ground;
    }

    fn esc_received(&mut self, byte: u8) {
        match byte {
            b'[' => {
                self.buf.push(byte);
                self.state = State::CsiEntry;
            }
            b'O' => {
                self.buf.push(byte);
                self.state = State::Ss3Pending;
            }
            0x1b => {
                // Double ESC: emit first ESC, start new ESC sequence
                self.emit_key(vec![0x1b], KeyKind::Escape);
                self.buf.clear();
                self.buf.push(0x1b);
                // state stays EscReceived
            }
            0x7f => {
                // ESC + DEL = Alt+Backspace (Option+Backspace)
                self.buf.push(byte);
                let raw = std::mem::take(&mut self.buf);
                self.emit_key(raw, KeyKind::Alt(Box::new(KeyKind::Backspace)));
                self.state = State::Ground;
            }
            // ESC + control byte = Alt+Control
            0x01..=0x1a => {
                self.buf.push(byte);
                let raw = std::mem::take(&mut self.buf);
                let letter = (b'a' + byte - 1) as char;
                self.emit_key(raw, KeyKind::Alt(Box::new(KeyKind::Control(letter))));
                self.state = State::Ground;
            }
            // ESC + printable = Alt+Char
            0x20..=0x7e => {
                self.buf.push(byte);
                let raw = std::mem::take(&mut self.buf);
                self.emit_key(raw, KeyKind::Alt(Box::new(KeyKind::Char(byte as char))));
                self.state = State::Ground;
            }
            _ => {
                // Unknown ESC sequence
                self.buf.push(byte);
                let raw = std::mem::take(&mut self.buf);
                self.emit_key(raw, KeyKind::Unknown);
                self.state = State::Ground;
            }
        }
    }

    fn csi_entry(&mut self, byte: u8) {
        match byte {
            b'M' => {
                self.buf.push(byte);
                self.state = State::MouseX10 { count: 0 };
            }
            b'<' => {
                self.buf.push(byte);
                self.state = State::SgrMouse;
            }
            b'A'..=b'D' | b'H' | b'F' => {
                // Direct final byte — arrow/home/end without params
                self.buf.push(byte);
                let raw = std::mem::take(&mut self.buf);
                let kind = classify_csi_final(byte, &[]);
                self.emit_key(raw, kind);
                self.state = State::Ground;
            }
            b'0'..=b'9' | b';' => {
                self.buf.push(byte);
                self.state = State::CsiParam;
            }
            // Any other letter terminates
            0x40..=0x7e => {
                self.buf.push(byte);
                let raw = std::mem::take(&mut self.buf);
                self.emit_key(raw, KeyKind::Unknown);
                self.state = State::Ground;
            }
            _ => {
                self.buf.push(byte);
                let raw = std::mem::take(&mut self.buf);
                self.emit_key(raw, KeyKind::Unknown);
                self.state = State::Ground;
            }
        }
    }

    fn csi_param(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' | b';' => {
                self.buf.push(byte);
            }
            b'~' => {
                self.buf.push(byte);
                let params = extract_csi_params(&self.buf);
                if params.first() == Some(&200) {
                    // Start bracketed paste
                    self.buf.clear();
                    self.paste_buf.clear();
                    self.state = State::PasteBuf;
                } else {
                    let raw = std::mem::take(&mut self.buf);
                    let kind = classify_tilde_key(&params);
                    self.emit_key(raw, kind);
                    self.state = State::Ground;
                }
            }
            // Final byte (letter): complete the CSI sequence
            0x40..=0x7e => {
                self.buf.push(byte);
                let params = extract_csi_params(&self.buf);
                let raw = std::mem::take(&mut self.buf);
                let kind = classify_csi_final(byte, &params);
                self.emit_key(raw, kind);
                self.state = State::Ground;
            }
            _ => {
                self.buf.push(byte);
                let raw = std::mem::take(&mut self.buf);
                self.emit_key(raw, KeyKind::Unknown);
                self.state = State::Ground;
            }
        }
    }

    fn ss3_pending(&mut self, byte: u8) {
        self.buf.push(byte);
        let raw = std::mem::take(&mut self.buf);
        let kind = match byte {
            b'A' => KeyKind::Arrow(Direction::Up),
            b'B' => KeyKind::Arrow(Direction::Down),
            b'C' => KeyKind::Arrow(Direction::Right),
            b'D' => KeyKind::Arrow(Direction::Left),
            b'H' => KeyKind::Nav(NavKey::Home),
            b'F' => KeyKind::Nav(NavKey::End),
            b'P' => KeyKind::Function(1),
            b'Q' => KeyKind::Function(2),
            b'R' => KeyKind::Function(3),
            b'S' => KeyKind::Function(4),
            _ => KeyKind::Unknown,
        };
        self.emit_key(raw, kind);
        self.state = State::Ground;
    }

    fn mouse_x10(&mut self, byte: u8, count: u8) {
        self.buf.push(byte);
        let new_count = count + 1;
        if new_count < 3 {
            self.state = State::MouseX10 { count: new_count };
            return;
        }
        // We have all 3 bytes: button, col, row
        let btn_byte = self.buf[self.buf.len() - 3];
        let col_byte = self.buf[self.buf.len() - 2];
        let row_byte = self.buf[self.buf.len() - 1];
        let code = btn_byte.wrapping_sub(32);
        let col = col_byte.wrapping_sub(33) as u16;
        let row = row_byte.wrapping_sub(33) as u16;
        let event = decode_x10_mouse(code, col, row);
        self.buf.clear();
        self.out.push_back(InputEvent::Mouse(event));
        self.state = State::Ground;
    }

    fn sgr_mouse(&mut self, byte: u8) {
        self.buf.push(byte);
        match byte {
            b'M' | b'm' => {
                // Parse SGR mouse: ESC [ < Pb ; Px ; Py M/m
                let is_release = byte == b'm';
                if let Some(event) = parse_sgr_mouse(&self.buf, is_release) {
                    self.buf.clear();
                    self.out.push_back(InputEvent::Mouse(event));
                } else {
                    let raw = std::mem::take(&mut self.buf);
                    self.emit_key(raw, KeyKind::Unknown);
                }
                self.state = State::Ground;
            }
            b'0'..=b'9' | b';' => {
                // Continue collecting params
            }
            _ => {
                let raw = std::mem::take(&mut self.buf);
                self.emit_key(raw, KeyKind::Unknown);
                self.state = State::Ground;
            }
        }
    }

    fn paste_buf(&mut self, byte: u8) {
        // Detect ESC[201~ end marker.
        // We accumulate raw bytes to detect the end sequence.
        self.buf.push(byte);

        // Check if buf ends with ESC[201~
        const END_MARKER: &[u8] = b"\x1b[201~";
        if self.buf.len() >= END_MARKER.len()
            && &self.buf[self.buf.len() - END_MARKER.len()..] == END_MARKER
        {
            // Remove the end marker bytes from paste content
            // Everything before the marker in buf is paste content
            let paste_bytes = &self.buf[..self.buf.len() - END_MARKER.len()];
            let text = String::from_utf8_lossy(paste_bytes).into_owned();
            self.out.push_back(InputEvent::Paste(text));
            self.buf.clear();
            self.paste_buf.clear();
            self.state = State::Ground;
        }
    }

    fn emit_key(&mut self, raw: Vec<u8>, kind: KeyKind) {
        self.out.push_back(InputEvent::Key(KeyInput { raw, kind }));
    }
}

impl Default for InputParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract numeric parameters from a CSI sequence buffer.
/// Buffer format: ESC [ <params> <final>
/// Returns the parsed numbers (semicolon-separated).
fn extract_csi_params(buf: &[u8]) -> Vec<u16> {
    // Find the params portion: after ESC[ and before the final byte
    let start = if buf.starts_with(b"\x1b[") { 2 } else { 0 };
    let end = buf.len().saturating_sub(1); // exclude final byte
    if start >= end {
        return vec![];
    }
    let params_str = &buf[start..end];
    params_str
        .split(|&b| b == b';')
        .map(|part| {
            part.iter().fold(0u16, |acc, &b| {
                if b.is_ascii_digit() {
                    acc.saturating_mul(10).saturating_add((b - b'0') as u16)
                } else {
                    acc
                }
            })
        })
        .collect()
}

/// Classify a CSI sequence ending with a letter (not ~).
fn classify_csi_final(final_byte: u8, params: &[u16]) -> KeyKind {
    let base = match final_byte {
        b'A' => KeyKind::Arrow(Direction::Up),
        b'B' => KeyKind::Arrow(Direction::Down),
        b'C' => KeyKind::Arrow(Direction::Right),
        b'D' => KeyKind::Arrow(Direction::Left),
        b'H' => KeyKind::Nav(NavKey::Home),
        b'F' => KeyKind::Nav(NavKey::End),
        _ => return KeyKind::Unknown,
    };

    // Check for modifier: CSI 1;mod X
    if params.len() >= 2 {
        let modifier = params[1];
        return wrap_modifier(base, modifier);
    }

    base
}

/// Classify a CSI sequence ending with ~.
fn classify_tilde_key(params: &[u16]) -> KeyKind {
    let code = params.first().copied().unwrap_or(0);
    let base = match code {
        2 => KeyKind::Nav(NavKey::Insert),
        3 => KeyKind::Nav(NavKey::Delete),
        5 => KeyKind::Nav(NavKey::PageUp),
        6 => KeyKind::Nav(NavKey::PageDown),
        15 => KeyKind::Function(5),
        17 => KeyKind::Function(6),
        18 => KeyKind::Function(7),
        19 => KeyKind::Function(8),
        20 => KeyKind::Function(9),
        21 => KeyKind::Function(10),
        23 => KeyKind::Function(11),
        24 => KeyKind::Function(12),
        _ => return KeyKind::Unknown,
    };

    // Check for modifier: CSI code;mod ~
    if params.len() >= 2 {
        let modifier = params[1];
        return wrap_modifier(base, modifier);
    }

    base
}

/// Wrap a KeyKind in modifier layers based on xterm modifier parameter.
/// Modifier param = 1 + bitmask: bit0=Shift, bit1=Alt, bit2=Ctrl.
fn wrap_modifier(base: KeyKind, modifier: u16) -> KeyKind {
    let bits = modifier.saturating_sub(1);
    let has_alt = bits & 2 != 0;
    // Shift and Ctrl don't change the KeyKind for our purposes —
    // they're encoded in the raw bytes which are forwarded as-is.
    // We only wrap Alt for semantic matching in hotkey/popup logic.
    if has_alt {
        KeyKind::Alt(Box::new(base))
    } else {
        base
    }
}

/// Decode X10 mouse button code into a MouseEvent.
fn decode_x10_mouse(code: u8, col: u16, row: u16) -> MouseEvent {
    let low_bits = code & 0x03;
    let is_drag = code & 32 != 0;
    let is_scroll = code & 64 != 0;

    if is_scroll {
        if low_bits == 0 {
            MouseEvent::ScrollUp { col, row }
        } else {
            MouseEvent::ScrollDown { col, row }
        }
    } else if is_drag {
        if low_bits == 3 {
            // Motion with no button pressed (?1003h all-motion tracking)
            MouseEvent::Move { col, row }
        } else {
            let button = match low_bits {
                0 => MouseButton::Left,
                1 => MouseButton::Middle,
                2 => MouseButton::Right,
                _ => MouseButton::Left,
            };
            MouseEvent::Drag { button, col, row }
        }
    } else if low_bits == 3 {
        MouseEvent::Up { col, row }
    } else {
        let button = match low_bits {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => MouseButton::Left,
        };
        MouseEvent::Down { button, col, row }
    }
}

/// Parse SGR mouse sequence. Buffer: ESC [ < Pb ; Px ; Py M/m
fn parse_sgr_mouse(buf: &[u8], is_release: bool) -> Option<MouseEvent> {
    // Find '<' and extract params after it
    let lt_pos = buf.iter().position(|&b| b == b'<')?;
    let params_end = buf.len() - 1; // exclude final M/m
    let params_str = &buf[lt_pos + 1..params_end];

    let parts: Vec<u16> = params_str
        .split(|&b| b == b';')
        .map(|part| {
            part.iter().fold(0u16, |acc, &b| {
                if b.is_ascii_digit() {
                    acc.saturating_mul(10).saturating_add((b - b'0') as u16)
                } else {
                    acc
                }
            })
        })
        .collect();

    if parts.len() < 3 {
        return None;
    }

    let code = parts[0];
    let col = parts[1].saturating_sub(1); // SGR uses 1-based
    let row = parts[2].saturating_sub(1);

    let low_bits = (code & 0x03) as u8;
    let is_drag = code & 32 != 0;
    let is_scroll = code & 64 != 0;

    if is_scroll {
        return Some(if low_bits == 0 {
            MouseEvent::ScrollUp { col, row }
        } else {
            MouseEvent::ScrollDown { col, row }
        });
    }

    if is_release {
        return Some(MouseEvent::Up { col, row });
    }

    if is_drag {
        if low_bits == 3 {
            // Motion with no button pressed (?1003h all-motion tracking)
            return Some(MouseEvent::Move { col, row });
        }
        let button = match low_bits {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => MouseButton::Left,
        };
        Some(MouseEvent::Drag { button, col, row })
    } else {
        let button = match low_bits {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => MouseButton::Left,
        };
        Some(MouseEvent::Down { button, col, row })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_backspace() {
        let mut p = InputParser::new();
        let events = p.feed(&[0x1b, 0x7f]);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Key(KeyInput {
                raw: vec![0x1b, 0x7f],
                kind: KeyKind::Alt(Box::new(KeyKind::Backspace)),
            })
        );
    }

    #[test]
    fn ctrl_b() {
        let mut p = InputParser::new();
        let events = p.feed(&[0x02]);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Key(KeyInput {
                raw: vec![0x02],
                kind: KeyKind::Control('b'),
            })
        );
    }

    #[test]
    fn arrow_up() {
        let mut p = InputParser::new();
        let events = p.feed(&[0x1b, b'[', b'A']);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Key(KeyInput {
                raw: vec![0x1b, b'[', b'A'],
                kind: KeyKind::Arrow(Direction::Up),
            })
        );
    }

    #[test]
    fn arrow_down_with_alt_modifier() {
        let mut p = InputParser::new();
        // CSI 1;3B = Alt+Down
        let events = p.feed(&[0x1b, b'[', b'1', b';', b'3', b'B']);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Key(KeyInput {
                raw: vec![0x1b, b'[', b'1', b';', b'3', b'B'],
                kind: KeyKind::Alt(Box::new(KeyKind::Arrow(Direction::Down))),
            })
        );
    }

    #[test]
    fn bare_esc_with_flush() {
        let mut p = InputParser::new();
        let events = p.feed(&[0x1b]);
        assert!(events.is_empty());
        assert!(p.has_pending());

        let events = p.flush();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Key(KeyInput {
                raw: vec![0x1b],
                kind: KeyKind::Escape,
            })
        );
    }

    #[test]
    fn printable_chars() {
        let mut p = InputParser::new();
        let events = p.feed(b"abc");
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0],
            InputEvent::Key(KeyInput {
                raw: vec![b'a'],
                kind: KeyKind::Char('a'),
            })
        );
    }

    #[test]
    fn enter_tab_backspace() {
        let mut p = InputParser::new();
        let events = p.feed(&[0x0d, 0x09, 0x7f]);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], InputEvent::Key(KeyInput { raw: vec![0x0d], kind: KeyKind::Enter }));
        assert_eq!(events[1], InputEvent::Key(KeyInput { raw: vec![0x09], kind: KeyKind::Tab }));
        assert_eq!(events[2], InputEvent::Key(KeyInput { raw: vec![0x7f], kind: KeyKind::Backspace }));
    }

    #[test]
    fn alt_char() {
        let mut p = InputParser::new();
        let events = p.feed(&[0x1b, b'd']);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Key(KeyInput {
                raw: vec![0x1b, b'd'],
                kind: KeyKind::Alt(Box::new(KeyKind::Char('d'))),
            })
        );
    }

    #[test]
    fn bracketed_paste() {
        let mut p = InputParser::new();
        let mut input = Vec::new();
        input.extend_from_slice(b"\x1b[200~");
        input.extend_from_slice(b"hello world");
        input.extend_from_slice(b"\x1b[201~");
        let events = p.feed(&input);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], InputEvent::Paste("hello world".to_string()));
    }

    #[test]
    fn x10_mouse_down_left() {
        let mut p = InputParser::new();
        // ESC [ M <button+32=32> <col+33=34> <row+33=35>  → button=0(left), col=1, row=2
        let events = p.feed(&[0x1b, b'[', b'M', 32, 34, 35]);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Mouse(MouseEvent::Down {
                button: MouseButton::Left,
                col: 1,
                row: 2,
            })
        );
    }

    #[test]
    fn sgr_mouse_down() {
        let mut p = InputParser::new();
        // ESC [ < 0;10;20 M  → left button down at col=9, row=19
        let events = p.feed(b"\x1b[<0;10;20M");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Mouse(MouseEvent::Down {
                button: MouseButton::Left,
                col: 9,
                row: 19,
            })
        );
    }

    #[test]
    fn sgr_mouse_release() {
        let mut p = InputParser::new();
        // ESC [ < 0;5;10 m  → release at col=4, row=9
        let events = p.feed(b"\x1b[<0;5;10m");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Mouse(MouseEvent::Up { col: 4, row: 9 })
        );
    }

    #[test]
    fn page_up_down() {
        let mut p = InputParser::new();
        let events = p.feed(b"\x1b[5~\x1b[6~");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], InputEvent::Key(KeyInput {
            raw: vec![0x1b, b'[', b'5', b'~'],
            kind: KeyKind::Nav(NavKey::PageUp),
        }));
        assert_eq!(events[1], InputEvent::Key(KeyInput {
            raw: vec![0x1b, b'[', b'6', b'~'],
            kind: KeyKind::Nav(NavKey::PageDown),
        }));
    }

    #[test]
    fn delete_insert() {
        let mut p = InputParser::new();
        let events = p.feed(b"\x1b[3~\x1b[2~");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], InputEvent::Key(KeyInput {
            raw: vec![0x1b, b'[', b'3', b'~'],
            kind: KeyKind::Nav(NavKey::Delete),
        }));
        assert_eq!(events[1], InputEvent::Key(KeyInput {
            raw: vec![0x1b, b'[', b'2', b'~'],
            kind: KeyKind::Nav(NavKey::Insert),
        }));
    }

    #[test]
    fn home_end() {
        let mut p = InputParser::new();
        let events = p.feed(b"\x1b[H\x1b[F");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], InputEvent::Key(KeyInput {
            raw: vec![0x1b, b'[', b'H'],
            kind: KeyKind::Nav(NavKey::Home),
        }));
        assert_eq!(events[1], InputEvent::Key(KeyInput {
            raw: vec![0x1b, b'[', b'F'],
            kind: KeyKind::Nav(NavKey::End),
        }));
    }

    #[test]
    fn ss3_function_keys() {
        let mut p = InputParser::new();
        // F1=ESC O P, F2=ESC O Q
        let events = p.feed(b"\x1bOP\x1bOQ");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], InputEvent::Key(KeyInput {
            raw: vec![0x1b, b'O', b'P'],
            kind: KeyKind::Function(1),
        }));
        assert_eq!(events[1], InputEvent::Key(KeyInput {
            raw: vec![0x1b, b'O', b'Q'],
            kind: KeyKind::Function(2),
        }));
    }

    #[test]
    fn partial_escape_across_feeds() {
        let mut p = InputParser::new();
        // Feed ESC alone
        let events = p.feed(&[0x1b]);
        assert!(events.is_empty());
        assert!(p.has_pending());

        // Feed [ A (rest of arrow up)
        let events = p.feed(&[b'[', b'A']);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Key(KeyInput {
                raw: vec![0x1b, b'[', b'A'],
                kind: KeyKind::Arrow(Direction::Up),
            })
        );
    }

    #[test]
    fn scroll_mouse_x10() {
        let mut p = InputParser::new();
        // Scroll up: code = 64 + 32 = 96, col=1+33=34, row=1+33=34
        let events = p.feed(&[0x1b, b'[', b'M', 96, 34, 34]);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Mouse(MouseEvent::ScrollUp { col: 1, row: 1 })
        );
    }

    #[test]
    fn ctrl_keys() {
        let mut p = InputParser::new();
        // Ctrl+A=0x01, Ctrl+H=0x08, Ctrl+Z=0x1a
        let events = p.feed(&[0x01, 0x08, 0x1a]);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], InputEvent::Key(KeyInput { raw: vec![0x01], kind: KeyKind::Control('a') }));
        assert_eq!(events[1], InputEvent::Key(KeyInput { raw: vec![0x08], kind: KeyKind::Control('h') }));
        assert_eq!(events[2], InputEvent::Key(KeyInput { raw: vec![0x1a], kind: KeyKind::Control('z') }));
    }

    #[test]
    fn double_esc() {
        let mut p = InputParser::new();
        let events = p.feed(&[0x1b, 0x1b]);
        // First ESC is emitted, second starts new pending
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], InputEvent::Key(KeyInput { raw: vec![0x1b], kind: KeyKind::Escape }));
        assert!(p.has_pending());

        let events = p.flush();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], InputEvent::Key(KeyInput { raw: vec![0x1b], kind: KeyKind::Escape }));
    }

    #[test]
    fn f5_function_key() {
        let mut p = InputParser::new();
        // F5 = CSI 15~
        let events = p.feed(b"\x1b[15~");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], InputEvent::Key(KeyInput {
            raw: b"\x1b[15~".to_vec(),
            kind: KeyKind::Function(5),
        }));
    }

    #[test]
    fn utf8_multibyte() {
        let mut p = InputParser::new();
        // Russian "Б" = 0xD0 0x91 (2-byte UTF-8)
        let events = p.feed(&[0xD0, 0x91]);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Key(KeyInput {
                raw: vec![0xD0, 0x91],
                kind: KeyKind::Char('Б'),
            })
        );
    }

    #[test]
    fn utf8_emoji() {
        let mut p = InputParser::new();
        // "😀" = 0xF0 0x9F 0x98 0x80 (4-byte UTF-8)
        let events = p.feed(&[0xF0, 0x9F, 0x98, 0x80]);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            InputEvent::Key(KeyInput {
                raw: vec![0xF0, 0x9F, 0x98, 0x80],
                kind: KeyKind::Char('😀'),
            })
        );
    }

    #[test]
    fn mouse_to_x10_bytes_roundtrip() {
        let event = MouseEvent::Down { button: MouseButton::Left, col: 5, row: 10 };
        let bytes = event.to_x10_bytes();
        assert_eq!(bytes[0], 0x1b);
        assert_eq!(bytes[1], b'[');
        assert_eq!(bytes[2], b'M');
        assert_eq!(bytes[3], 32); // left=0, +32
        assert_eq!(bytes[4], 38); // col=5, +33
        assert_eq!(bytes[5], 43); // row=10, +33
    }
}
