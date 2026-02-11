/// A parsed terminal input event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// Keyboard input with semantic classification and raw bytes.
    Key(KeyInput),
    /// Mouse event (X10 or SGR protocol).
    Mouse(MouseEvent),
    /// Bracketed paste content (text between ESC[200~ and ESC[201~).
    Paste(String),
}

/// Keyboard input with semantic classification AND raw bytes for lossless forwarding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInput {
    /// Raw bytes exactly as received from terminal — for forwarding to PTY.
    pub raw: Vec<u8>,
    /// Semantic classification of the key press.
    pub kind: KeyKind,
}

/// Semantic classification of a keyboard input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyKind {
    /// Printable character (ASCII or multi-byte UTF-8).
    Char(char),
    /// Ctrl+letter: Ctrl+A=0x01 .. Ctrl+Z=0x1a. Stores the lowercase letter.
    Control(char),
    /// Bare ESC (after timeout — no follow-up byte arrived).
    Escape,
    /// Enter / CR (0x0d).
    Enter,
    /// Tab / HT (0x09).
    Tab,
    /// Backspace / DEL (0x7f).
    Backspace,
    /// Arrow key.
    Arrow(Direction),
    /// Navigation key (Home, End, Insert, Delete, PageUp, PageDown).
    Nav(NavKey),
    /// Function key F1–F12.
    Function(u8),
    /// Alt (ESC prefix) + another key. E.g. Alt(Backspace) = Option+Backspace.
    Alt(Box<KeyKind>),
    /// Unrecognized CSI/SS3 sequence (raw bytes preserved in `KeyInput::raw`).
    Unknown,
}

/// Arrow key direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Right,
    Left,
}

/// Navigation key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKey {
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
}

/// Mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// Mouse event parsed from X10 or SGR protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEvent {
    Down { button: MouseButton, col: u16, row: u16 },
    Up { col: u16, row: u16 },
    Drag { button: MouseButton, col: u16, row: u16 },
    /// Mouse motion without any button pressed (from ?1003h all-motion tracking).
    Move { col: u16, row: u16 },
    ScrollUp { col: u16, row: u16 },
    ScrollDown { col: u16, row: u16 },
}

impl MouseEvent {
    /// Encode as X10 mouse protocol bytes: ESC [ M <button+32> <col+33> <row+33>.
    pub fn to_x10_bytes(&self) -> [u8; 6] {
        let (button_code, col, row) = match *self {
            MouseEvent::Down { button, col, row } => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                };
                (b, col, row)
            }
            MouseEvent::Up { col, row } => (3, col, row),
            MouseEvent::Drag { button, col, row } => {
                let b = match button {
                    MouseButton::Left => 32,
                    MouseButton::Middle => 33,
                    MouseButton::Right => 34,
                };
                (b, col, row)
            }
            MouseEvent::Move { col, row } => (35, col, row), // motion, no button
            MouseEvent::ScrollUp { col, row } => (64, col, row),
            MouseEvent::ScrollDown { col, row } => (65, col, row),
        };
        [
            0x1b,
            b'[',
            b'M',
            (button_code + 32) as u8,
            (col + 33) as u8,
            (row + 33) as u8,
        ]
    }

    /// True if this is a scroll event.
    pub fn is_scroll(&self) -> bool {
        matches!(self, MouseEvent::ScrollUp { .. } | MouseEvent::ScrollDown { .. })
    }

    /// Column and row of this event.
    pub fn position(&self) -> (u16, u16) {
        match *self {
            MouseEvent::Down { col, row, .. }
            | MouseEvent::Up { col, row }
            | MouseEvent::Drag { col, row, .. }
            | MouseEvent::Move { col, row }
            | MouseEvent::ScrollUp { col, row }
            | MouseEvent::ScrollDown { col, row } => (col, row),
        }
    }
}
