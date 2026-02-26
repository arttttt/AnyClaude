# GPU Terminal Specification — AnyClaude

## 1. Обзор

### Цель
Заменить текстовый TUI-рендерер (ratatui + alacritty_terminal) на кастомный GPU-рендерер с поддержкой variable-width шрифтов и системой панелей.

### Текущий стек
```
PTY (portable-pty) → alacritty_terminal (VT parser) → TermCell grid → ratatui → stdout
```

**Ограничения текущего стека:**
- Monospace-only рендеринг через ratatui
- Фиксированная сетка ячеек (row/col), нет пиксельного позиционирования
- Нет поддержки панелей (layout: header + body + footer, одна терминальная панель)
- Нет GPU-ускорения, рендеринг через escape sequences в stdout
- Зависимость от alacritty_terminal — тяжёлый полный VT-эмулятор, избыточный для Claude Code

### Целевой стек
```
PTY (portable-pty) → term_core (минимальный VT parser) → TextRun grid → wgpu → GPU
```

**Преимущества:**
- Variable-width шрифты с пиксельным позиционированием
- GPU-ускоренный рендеринг (Metal на macOS, Vulkan на Linux)
- Система панелей (BSP tree)
- Минимальный VT-парсер — только то, что нужно для Claude Code (ink-based TUI)
- Минимум зависимостей

---

## 2. Зависимости

### Внешние (только 3 крейта)

| Крейт | Версия | Назначение | Почему нельзя написать самим |
|-------|--------|------------|------------------------------|
| `wgpu` | 24 | GPU abstraction (Metal/Vulkan/DX12) | Тысячи строк платформенного кода, активная поддержка драйверов |
| `winit` | 0.30 | Окна, события, DPI | Platform-specific windowing (Cocoa, Wayland, X11) |
| `cosmic-text` | 0.14 | Text shaping для variable-width | Unicode shaping (HarfBuzz-level complexity), BiDi, font fallback |

### Удаляемые зависимости

| Крейт | Причина удаления |
|-------|-----------------|
| `ratatui` | Заменяется GPU рендерером |
| `alacritty_terminal` | Заменяется term_core |
| `crossterm` | Заменяется winit (события) |
| `signal-hook` | Встраивается в winit event loop |

### Остающиеся без изменений

portable-pty, tokio, axum, reqwest, clap, serde/serde_json/toml, dirs, parking_lot, arboard, anyhow, tempfile, uuid, term_input (адаптируется).

---

## 3. Структура крейтов

```
Cargo.toml (workspace)
├── src/                    # anyclaude — main app
├── crates/
│   ├── term_input/         # [существующий] обработка ввода с /dev/tty
│   ├── term_core/          # [НОВЫЙ] минимальный VT-парсер + variable-width grid
│   ├── term_gpu/           # [НОВЫЙ] wgpu рендерер, glyph atlas, шейдеры
│   └── term_layout/        # [НОВЫЙ] BSP panel manager
```

### Зависимости между крейтами

```
term_core         (0 внешних deps, только std)
    ↑
term_gpu          → wgpu, cosmic-text, term_core
    ↑
term_layout       → term_gpu, term_core
    ↑
anyclaude         → term_layout, term_gpu, term_core, term_input
```

### Cargo.toml крейтов

#### crates/term_core/Cargo.toml
```toml
[package]
name = "term_core"
version = "0.1.0"
edition = "2021"

[lints]
workspace = true

# Ноль внешних зависимостей — только std
```

#### crates/term_gpu/Cargo.toml
```toml
[package]
name = "term_gpu"
version = "0.1.0"
edition = "2021"

[lints]
workspace = true

[dependencies]
term_core = { path = "../term_core" }
wgpu = "24"
winit = "0.30"
cosmic-text = "0.14"
pollster = "0.4"       # block_on для async wgpu init
```

#### crates/term_layout/Cargo.toml
```toml
[package]
name = "term_layout"
version = "0.1.0"
edition = "2021"

[lints]
workspace = true

[dependencies]
term_core = { path = "../term_core" }
term_gpu = { path = "../term_gpu" }
```

#### Обновлённый корневой Cargo.toml (изменения)
```toml
[workspace]
members = [".", "crates/term_input", "crates/term_core", "crates/term_gpu", "crates/term_layout"]

[dependencies]
# УДАЛИТЬ:
# ratatui = "0.30"
# alacritty_terminal = "0.25"
# crossterm = "0.29"
# signal-hook = "0.4"

# ДОБАВИТЬ:
term_core = { path = "crates/term_core" }
term_gpu = { path = "crates/term_gpu" }
term_layout = { path = "crates/term_layout" }

# ОСТАВИТЬ БЕЗ ИЗМЕНЕНИЙ:
# portable-pty, tokio, axum, reqwest, clap, serde, и т.д.
```

---

## 4. term_core: Минимальный VT-парсер

### 4.1 Структура файлов

```
crates/term_core/src/
├── lib.rs          # Публичный API, реэкспорт
├── parser.rs       # VT state machine
├── grid.rs         # Variable-width grid (TextRun-based)
├── emulator.rs     # VtEmulator — реализация трейта TerminalEmulator
├── color.rs        # TermColor, ANSI palette
└── attrs.rs        # TextAttrs (bold, italic, etc.)
```

### 4.2 Поддерживаемые escape sequences

Только то, что использует Claude Code (ink-based TUI):

#### CSI sequences (`ESC [`)
| Sequence | Код | Назначение |
|----------|-----|-----------|
| CUU | `ESC[{n}A` | Cursor Up |
| CUD | `ESC[{n}B` | Cursor Down |
| CUF | `ESC[{n}C` | Cursor Forward |
| CUB | `ESC[{n}D` | Cursor Back |
| CUP | `ESC[{r};{c}H` | Cursor Position |
| CHA | `ESC[{n}G` | Cursor Horizontal Absolute |
| ED | `ESC[{n}J` | Erase Display (0=to end, 1=to start, 2=all, 3=scrollback) |
| EL | `ESC[{n}K` | Erase Line (0=to end, 1=to start, 2=all) |
| IL | `ESC[{n}L` | Insert Lines |
| DL | `ESC[{n}M` | Delete Lines |
| SGR | `ESC[{...}m` | Set Graphics Rendition (цвета, стили) |
| DECSTBM | `ESC[{t};{b}r` | Set Scroll Region |
| DECSET | `ESC[?{n}h` | Set DEC Private Mode |
| DECRST | `ESC[?{n}l` | Reset DEC Private Mode |
| DSR | `ESC[{n}n` | Device Status Report |
| SU | `ESC[{n}S` | Scroll Up |
| SD | `ESC[{n}T` | Scroll Down |

#### SGR parameters (`ESC[{...}m`)
| Код | Значение |
|-----|----------|
| 0 | Reset |
| 1 | Bold |
| 2 | Faint |
| 3 | Italic |
| 4 | Underline |
| 7 | Inverse |
| 9 | Strikethrough |
| 22 | Normal intensity |
| 23 | Not italic |
| 24 | Not underline |
| 27 | Not inverse |
| 29 | Not strikethrough |
| 30-37 | Foreground (standard) |
| 38;5;{n} | Foreground (256-color) |
| 38;2;{r};{g};{b} | Foreground (truecolor) |
| 39 | Default foreground |
| 40-47 | Background (standard) |
| 48;5;{n} | Background (256-color) |
| 48;2;{r};{g};{b} | Background (truecolor) |
| 49 | Default background |
| 90-97 | Foreground (bright) |
| 100-107 | Background (bright) |

#### DEC Private Modes (`ESC[?{n}h/l`)
| Код | Режим | Назначение |
|-----|-------|-----------|
| 1 | DECCKM | Application cursor keys |
| 25 | DECTCEM | Cursor visible/hidden |
| 47 | Alt screen (save) | Alternate screen buffer |
| 1000 | X10 mouse | Basic mouse tracking |
| 1002 | Button event | Mouse button events |
| 1003 | Any event | All mouse events |
| 1006 | SGR mouse | SGR mouse encoding |
| 1049 | Alt screen (save+clear) | Alternate screen buffer + clear |
| 2004 | Bracketed paste | Bracketed paste mode |

#### OSC sequences (`ESC ]`)
| Sequence | Назначение |
|----------|-----------|
| `ESC]0;{title}ST` | Set window title |
| `ESC]2;{title}ST` | Set window title |

#### Simple escape sequences
| Sequence | Назначение |
|----------|-----------|
| `ESC 7` | Save cursor (DECSC) |
| `ESC 8` | Restore cursor (DECRC) |
| `ESC M` | Reverse index (scroll down) |
| `ESC c` | Full reset (RIS) |

#### Control characters
| Byte | Назначение |
|------|-----------|
| 0x07 (BEL) | Bell |
| 0x08 (BS) | Backspace |
| 0x09 (HT) | Tab |
| 0x0A (LF) | Line feed |
| 0x0D (CR) | Carriage return |

### 4.3 Что НЕ поддерживается

- DCS, SOS, PM, APC sequences
- Character sets (G0/G1/G2/G3, SI/SO)
- VT52 compatibility mode
- Двойная ширина/высота строк (DECDWL, DECDHL)
- Tab stops (HTS, TBC) — используем фиксированный tab = 8
- Printer control (MC)
- Soft fonts (DECDLD)
- Rectangular area operations (DECRARA, DECCRA)
- Macro sequences

### 4.4 Публичный API (lib.rs)

```rust
pub mod parser;
pub mod grid;
pub mod emulator;
pub mod color;
pub mod attrs;

pub use color::TermColor;
pub use attrs::TextAttrs;
pub use grid::{Grid, Line, TextRun, PixelPos};
pub use emulator::{TerminalEmulator, CursorState, CursorStyle, RenderSnapshot};
pub use parser::{Parser, Action};

/// Создать эмулятор терминала
pub fn create_emulator(width_px: u32, height_px: u32, scrollback: usize) -> Box<dyn TerminalEmulator> {
    Box::new(emulator::VtEmulator::new(width_px, height_px, scrollback))
}
```

### 4.5 Цвета и атрибуты (color.rs, attrs.rs)

```rust
// color.rs
/// Цвет терминала — совместим с текущим TermColor из emulator/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TermColor {
    #[default]
    Default,
    Indexed(u8),       // 0-255
    Rgb(u8, u8, u8),   // True color
}

impl TermColor {
    /// Стандартная ANSI палитра (16 цветов)
    pub const BLACK: Self = Self::Indexed(0);
    pub const RED: Self = Self::Indexed(1);
    pub const GREEN: Self = Self::Indexed(2);
    pub const YELLOW: Self = Self::Indexed(3);
    pub const BLUE: Self = Self::Indexed(4);
    pub const MAGENTA: Self = Self::Indexed(5);
    pub const CYAN: Self = Self::Indexed(6);
    pub const WHITE: Self = Self::Indexed(7);

    /// Конвертация в RGBA f32 для GPU
    pub fn to_rgba(&self, palette: &AnsiPalette) -> [f32; 4] {
        match *self {
            Self::Default => [1.0, 1.0, 1.0, 1.0],
            Self::Indexed(idx) => palette.color(idx),
            Self::Rgb(r, g, b) => [
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
                1.0,
            ],
        }
    }
}

/// ANSI 256-color palette
pub struct AnsiPalette {
    colors: [[f32; 4]; 256],
}

impl AnsiPalette {
    pub fn default_dark() -> Self {
        let mut colors = [[0.0f32; 4]; 256];
        // Standard 16 colors (dark theme)
        let base: [[u8; 3]; 16] = [
            [0x1d, 0x1f, 0x21], // 0 black
            [0xcc, 0x66, 0x66], // 1 red
            [0xb5, 0xbd, 0x68], // 2 green
            [0xf0, 0xc6, 0x74], // 3 yellow
            [0x81, 0xa2, 0xbe], // 4 blue
            [0xb2, 0x94, 0xbb], // 5 magenta
            [0x8a, 0xbe, 0xb7], // 6 cyan
            [0xc5, 0xc8, 0xc6], // 7 white
            [0x96, 0x98, 0x96], // 8 bright black
            [0xde, 0x93, 0x5f], // 9 bright red
            [0xa3, 0xbe, 0x8c], // 10 bright green
            [0xe5, 0xc0, 0x7b], // 11 bright yellow
            [0x7d, 0xae, 0xa3], // 12 bright blue
            [0xc7, 0x8d, 0xd4], // 13 bright magenta
            [0x70, 0xc0, 0xba], // 14 bright cyan
            [0xff, 0xff, 0xff], // 15 bright white
        ];
        for (i, rgb) in base.iter().enumerate() {
            colors[i] = [rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0, 1.0];
        }
        // 216 color cube (indices 16-231)
        for i in 0..216 {
            let r = (i / 36) % 6;
            let g = (i / 6) % 6;
            let b = i % 6;
            let to_f = |v: usize| if v == 0 { 0.0 } else { (55 + 40 * v) as f32 / 255.0 };
            colors[16 + i] = [to_f(r), to_f(g), to_f(b), 1.0];
        }
        // Grayscale ramp (indices 232-255)
        for i in 0..24 {
            let v = (8 + 10 * i) as f32 / 255.0;
            colors[232 + i] = [v, v, v, 1.0];
        }
        Self { colors }
    }

    pub fn color(&self, idx: u8) -> [f32; 4] {
        self.colors[idx as usize]
    }
}
```

```rust
// attrs.rs
/// Текстовые атрибуты
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TextAttrs {
    bits: u8,
}

impl TextAttrs {
    pub const BOLD: u8      = 1 << 0;
    pub const FAINT: u8     = 1 << 1;
    pub const ITALIC: u8    = 1 << 2;
    pub const UNDERLINE: u8 = 1 << 3;
    pub const INVERSE: u8   = 1 << 4;
    pub const STRIKE: u8    = 1 << 5;
    pub const BLINK: u8     = 1 << 6;

    pub fn empty() -> Self { Self { bits: 0 } }
    pub fn bold(&self) -> bool { self.bits & Self::BOLD != 0 }
    pub fn faint(&self) -> bool { self.bits & Self::FAINT != 0 }
    pub fn italic(&self) -> bool { self.bits & Self::ITALIC != 0 }
    pub fn underline(&self) -> bool { self.bits & Self::UNDERLINE != 0 }
    pub fn inverse(&self) -> bool { self.bits & Self::INVERSE != 0 }
    pub fn strike(&self) -> bool { self.bits & Self::STRIKE != 0 }

    pub fn set(&mut self, flag: u8) { self.bits |= flag; }
    pub fn clear(&mut self, flag: u8) { self.bits &= !flag; }
    pub fn reset(&mut self) { self.bits = 0; }
}
```

### 4.6 VT Parser (parser.rs)

```rust
// parser.rs
//! Минимальный VT/ANSI parser — state machine для Claude Code

use crate::{TermColor, TextAttrs};

const MAX_PARAMS: usize = 16;
const MAX_OSC: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    Utf8_2(u8),          // 1 lead byte accumulated
    Utf8_3(u8, u8),      // 2 bytes accumulated
    Utf8_4(u8, u8, u8),  // 3 bytes accumulated
}

/// Действия парсера — передаются в Grid/Emulator
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Printable character
    Print(char),
    /// Bell
    Bell,
    /// Backspace
    Backspace,
    /// Tab
    Tab,
    /// Line feed (newline)
    LineFeed,
    /// Carriage return
    CarriageReturn,
    /// Cursor up N
    CursorUp(u16),
    /// Cursor down N
    CursorDown(u16),
    /// Cursor forward N
    CursorForward(u16),
    /// Cursor back N
    CursorBack(u16),
    /// Cursor position (row, col) — 1-based
    CursorPosition { row: u16, col: u16 },
    /// Cursor horizontal absolute (col) — 1-based
    CursorColumn(u16),
    /// Erase display
    EraseDisplay(EraseMode),
    /// Erase line
    EraseLine(EraseMode),
    /// Insert lines
    InsertLines(u16),
    /// Delete lines
    DeleteLines(u16),
    /// Scroll up
    ScrollUp(u16),
    /// Scroll down
    ScrollDown(u16),
    /// Set graphics rendition
    SetAttr(SgrAction),
    /// Set scroll region (top, bottom) — 1-based
    SetScrollRegion { top: u16, bottom: u16 },
    /// DEC private mode set
    DecModeSet(u16),
    /// DEC private mode reset
    DecModeReset(u16),
    /// Device status report
    DeviceStatusReport(u16),
    /// Save cursor
    SaveCursor,
    /// Restore cursor
    RestoreCursor,
    /// Reverse index (scroll down at top)
    ReverseIndex,
    /// Full reset
    FullReset,
    /// Set window title
    SetTitle(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraseMode {
    ToEnd = 0,
    ToStart = 1,
    All = 2,
    Scrollback = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SgrAction {
    Reset,
    SetAttr(u8),       // TextAttrs flag to set
    ClearAttr(u8),     // TextAttrs flag to clear
    Foreground(TermColor),
    Background(TermColor),
    DefaultForeground,
    DefaultBackground,
}

pub struct Parser {
    state: State,
    params: [u16; MAX_PARAMS],
    param_count: usize,
    current_param: u16,
    /// '?' prefix for DEC private modes
    private_mode: bool,
    /// Intermediate bytes
    intermediate: u8,
    /// OSC string accumulator
    osc_buf: [u8; MAX_OSC],
    osc_len: usize,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            params: [0; MAX_PARAMS],
            param_count: 0,
            current_param: 0,
            private_mode: false,
            intermediate: 0,
            osc_buf: [0; MAX_OSC],
            osc_len: 0,
        }
    }

    /// Парсит bytes, вызывает callback для каждого действия.
    /// Zero-allocation для стандартных sequence.
    pub fn advance<F: FnMut(Action)>(&mut self, input: &[u8], mut emit: F) {
        for &byte in input {
            self.feed(byte, &mut emit);
        }
    }

    fn feed<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        // ESC и C0 обрабатываются из любого состояния (кроме OSC для ESC)
        if byte == 0x1B && self.state != State::OscString {
            self.state = State::Escape;
            return;
        }

        match self.state {
            State::Ground => self.ground(byte, emit),
            State::Escape => self.escape(byte, emit),
            State::CsiEntry => self.csi_entry(byte, emit),
            State::CsiParam => self.csi_param(byte, emit),
            State::CsiIntermediate => self.csi_intermediate(byte, emit),
            State::CsiIgnore => self.csi_ignore(byte),
            State::OscString => self.osc_string(byte, emit),
            State::Utf8_2(b0) => self.utf8_cont_2(b0, byte, emit),
            State::Utf8_3(b0, b1) => self.utf8_cont_3(b0, b1, byte, emit),
            State::Utf8_4(b0, b1, b2) => self.utf8_cont_4(b0, b1, b2, byte, emit),
        }
    }

    fn ground<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            0x07 => emit(Action::Bell),
            0x08 => emit(Action::Backspace),
            0x09 => emit(Action::Tab),
            0x0A | 0x0B | 0x0C => emit(Action::LineFeed),
            0x0D => emit(Action::CarriageReturn),
            0x1B => self.state = State::Escape,
            0x20..=0x7E => emit(Action::Print(byte as char)),
            0xC0..=0xDF => self.state = State::Utf8_2(byte),
            0xE0..=0xEF => self.state = State::Utf8_3(byte, 0),
            0xF0..=0xF4 => self.state = State::Utf8_4(byte, 0, 0),
            _ => {} // Ignore other C0/C1 and invalid bytes
        }
    }

    fn utf8_cont_2<F: FnMut(Action)>(&mut self, b0: u8, byte: u8, emit: &mut F) {
        if byte & 0xC0 == 0x80 {
            let cp = ((b0 as u32 & 0x1F) << 6) | (byte as u32 & 0x3F);
            if let Some(c) = char::from_u32(cp) {
                emit(Action::Print(c));
            }
        }
        self.state = State::Ground;
    }

    fn utf8_cont_3<F: FnMut(Action)>(&mut self, b0: u8, b1: u8, byte: u8, emit: &mut F) {
        if b1 == 0 {
            // Waiting for second continuation byte
            self.state = State::Utf8_3(b0, byte);
            return;
        }
        if byte & 0xC0 == 0x80 {
            let cp = ((b0 as u32 & 0x0F) << 12)
                | ((b1 as u32 & 0x3F) << 6)
                | (byte as u32 & 0x3F);
            if let Some(c) = char::from_u32(cp) {
                emit(Action::Print(c));
            }
        }
        self.state = State::Ground;
    }

    fn utf8_cont_4<F: FnMut(Action)>(&mut self, b0: u8, b1: u8, b2: u8, byte: u8, emit: &mut F) {
        if b1 == 0 {
            self.state = State::Utf8_4(b0, byte, 0);
            return;
        }
        if b2 == 0 {
            self.state = State::Utf8_4(b0, b1, byte);
            return;
        }
        if byte & 0xC0 == 0x80 {
            let cp = ((b0 as u32 & 0x07) << 18)
                | ((b1 as u32 & 0x3F) << 12)
                | ((b2 as u32 & 0x3F) << 6)
                | (byte as u32 & 0x3F);
            if let Some(c) = char::from_u32(cp) {
                emit(Action::Print(c));
            }
        }
        self.state = State::Ground;
    }

    fn escape<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            b'[' => {
                self.reset_csi();
                self.state = State::CsiEntry;
            }
            b']' => {
                self.osc_len = 0;
                self.state = State::OscString;
            }
            b'7' => { emit(Action::SaveCursor); self.state = State::Ground; }
            b'8' => { emit(Action::RestoreCursor); self.state = State::Ground; }
            b'M' => { emit(Action::ReverseIndex); self.state = State::Ground; }
            b'c' => { emit(Action::FullReset); self.state = State::Ground; }
            _ => self.state = State::Ground,
        }
    }

    fn csi_entry<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            b'?' => { self.private_mode = true; self.state = State::CsiParam; }
            b'0'..=b'9' => {
                self.current_param = (byte - b'0') as u16;
                self.state = State::CsiParam;
            }
            b';' => {
                self.push_param();
                self.state = State::CsiParam;
            }
            0x40..=0x7E => {
                self.push_param();
                self.dispatch_csi(byte, emit);
                self.state = State::Ground;
            }
            _ => self.state = State::CsiIgnore,
        }
    }

    fn csi_param<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            b'0'..=b'9' => {
                self.current_param = self.current_param
                    .saturating_mul(10)
                    .saturating_add((byte - b'0') as u16);
            }
            b';' => self.push_param(),
            0x20..=0x2F => {
                self.intermediate = byte;
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => {
                self.push_param();
                self.dispatch_csi(byte, emit);
                self.state = State::Ground;
            }
            _ => self.state = State::CsiIgnore,
        }
    }

    fn csi_intermediate<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            0x20..=0x2F => { /* accumulate */ }
            0x40..=0x7E => {
                self.push_param();
                // CSI with intermediates — mostly ignored
                self.state = State::Ground;
            }
            _ => self.state = State::CsiIgnore,
        }
    }

    fn csi_ignore(&mut self, byte: u8) {
        if (0x40..=0x7E).contains(&byte) {
            self.state = State::Ground;
        }
    }

    fn osc_string<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            0x07 => {
                // BEL terminates OSC
                self.dispatch_osc(emit);
                self.state = State::Ground;
            }
            0x1B => {
                // ESC — check if followed by \ (ST)
                // For simplicity, treat ESC as terminator
                self.dispatch_osc(emit);
                self.state = State::Escape;
            }
            0x9C => {
                // ST (C1)
                self.dispatch_osc(emit);
                self.state = State::Ground;
            }
            _ => {
                if self.osc_len < MAX_OSC {
                    self.osc_buf[self.osc_len] = byte;
                    self.osc_len += 1;
                }
            }
        }
    }

    fn reset_csi(&mut self) {
        self.param_count = 0;
        self.current_param = 0;
        self.private_mode = false;
        self.intermediate = 0;
    }

    fn push_param(&mut self) {
        if self.param_count < MAX_PARAMS {
            self.params[self.param_count] = self.current_param;
            self.param_count += 1;
        }
        self.current_param = 0;
    }

    fn param(&self, idx: usize, default: u16) -> u16 {
        if idx < self.param_count && self.params[idx] != 0 {
            self.params[idx]
        } else {
            default
        }
    }

    fn dispatch_csi<F: FnMut(Action)>(&self, final_byte: u8, emit: &mut F) {
        if self.private_mode {
            self.dispatch_dec_mode(final_byte, emit);
            return;
        }

        match final_byte {
            b'A' => emit(Action::CursorUp(self.param(0, 1))),
            b'B' => emit(Action::CursorDown(self.param(0, 1))),
            b'C' => emit(Action::CursorForward(self.param(0, 1))),
            b'D' => emit(Action::CursorBack(self.param(0, 1))),
            b'G' => emit(Action::CursorColumn(self.param(0, 1))),
            b'H' | b'f' => emit(Action::CursorPosition {
                row: self.param(0, 1),
                col: self.param(1, 1),
            }),
            b'J' => emit(Action::EraseDisplay(self.erase_mode(0))),
            b'K' => emit(Action::EraseLine(self.erase_mode(0))),
            b'L' => emit(Action::InsertLines(self.param(0, 1))),
            b'M' => emit(Action::DeleteLines(self.param(0, 1))),
            b'S' => emit(Action::ScrollUp(self.param(0, 1))),
            b'T' => emit(Action::ScrollDown(self.param(0, 1))),
            b'm' => self.dispatch_sgr(emit),
            b'n' => emit(Action::DeviceStatusReport(self.param(0, 0))),
            b'r' => emit(Action::SetScrollRegion {
                top: self.param(0, 1),
                bottom: self.param(1, u16::MAX),
            }),
            _ => {} // Unsupported — ignore
        }
    }

    fn dispatch_dec_mode<F: FnMut(Action)>(&self, final_byte: u8, emit: &mut F) {
        for i in 0..self.param_count {
            let mode = self.params[i];
            match final_byte {
                b'h' => emit(Action::DecModeSet(mode)),
                b'l' => emit(Action::DecModeReset(mode)),
                _ => {}
            }
        }
    }

    fn dispatch_sgr<F: FnMut(Action)>(&self, emit: &mut F) {
        if self.param_count == 0 {
            emit(Action::SetAttr(SgrAction::Reset));
            return;
        }
        let mut i = 0;
        while i < self.param_count {
            let p = self.params[i];
            match p {
                0 => emit(Action::SetAttr(SgrAction::Reset)),
                1 => emit(Action::SetAttr(SgrAction::SetAttr(TextAttrs::BOLD))),
                2 => emit(Action::SetAttr(SgrAction::SetAttr(TextAttrs::FAINT))),
                3 => emit(Action::SetAttr(SgrAction::SetAttr(TextAttrs::ITALIC))),
                4 => emit(Action::SetAttr(SgrAction::SetAttr(TextAttrs::UNDERLINE))),
                5 => emit(Action::SetAttr(SgrAction::SetAttr(TextAttrs::BLINK))),
                7 => emit(Action::SetAttr(SgrAction::SetAttr(TextAttrs::INVERSE))),
                9 => emit(Action::SetAttr(SgrAction::SetAttr(TextAttrs::STRIKE))),
                22 => {
                    emit(Action::SetAttr(SgrAction::ClearAttr(TextAttrs::BOLD)));
                    emit(Action::SetAttr(SgrAction::ClearAttr(TextAttrs::FAINT)));
                }
                23 => emit(Action::SetAttr(SgrAction::ClearAttr(TextAttrs::ITALIC))),
                24 => emit(Action::SetAttr(SgrAction::ClearAttr(TextAttrs::UNDERLINE))),
                27 => emit(Action::SetAttr(SgrAction::ClearAttr(TextAttrs::INVERSE))),
                29 => emit(Action::SetAttr(SgrAction::ClearAttr(TextAttrs::STRIKE))),
                30..=37 => emit(Action::SetAttr(SgrAction::Foreground(TermColor::Indexed(p as u8 - 30)))),
                38 => {
                    if let Some(color) = self.parse_extended_color(&mut i) {
                        emit(Action::SetAttr(SgrAction::Foreground(color)));
                    }
                }
                39 => emit(Action::SetAttr(SgrAction::DefaultForeground)),
                40..=47 => emit(Action::SetAttr(SgrAction::Background(TermColor::Indexed(p as u8 - 40)))),
                48 => {
                    if let Some(color) = self.parse_extended_color(&mut i) {
                        emit(Action::SetAttr(SgrAction::Background(color)));
                    }
                }
                49 => emit(Action::SetAttr(SgrAction::DefaultBackground)),
                90..=97 => emit(Action::SetAttr(SgrAction::Foreground(TermColor::Indexed(p as u8 - 90 + 8)))),
                100..=107 => emit(Action::SetAttr(SgrAction::Background(TermColor::Indexed(p as u8 - 100 + 8)))),
                _ => {} // Unknown — ignore
            }
            i += 1;
        }
    }

    fn parse_extended_color(&self, i: &mut usize) -> Option<TermColor> {
        if *i + 1 >= self.param_count { return None; }
        match self.params[*i + 1] {
            5 => {
                // 256-color: 38;5;N
                if *i + 2 < self.param_count {
                    *i += 2;
                    Some(TermColor::Indexed(self.params[*i] as u8))
                } else {
                    None
                }
            }
            2 => {
                // Truecolor: 38;2;R;G;B
                if *i + 4 < self.param_count {
                    *i += 4;
                    Some(TermColor::Rgb(
                        self.params[*i - 2] as u8,
                        self.params[*i - 1] as u8,
                        self.params[*i] as u8,
                    ))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn erase_mode(&self, param_idx: usize) -> EraseMode {
        match self.param(param_idx, 0) {
            0 => EraseMode::ToEnd,
            1 => EraseMode::ToStart,
            2 => EraseMode::All,
            3 => EraseMode::Scrollback,
            _ => EraseMode::ToEnd,
        }
    }

    fn dispatch_osc<F: FnMut(Action)>(&self, emit: &mut F) {
        let data = &self.osc_buf[..self.osc_len];
        // Find first ';' separator
        if let Some(sep) = data.iter().position(|&b| b == b';') {
            let cmd = &data[..sep];
            let payload = &data[sep + 1..];
            match cmd {
                b"0" | b"2" => {
                    if let Ok(title) = std::str::from_utf8(payload) {
                        emit(Action::SetTitle(title.to_string()));
                    }
                }
                _ => {} // Other OSC — ignore
            }
        }
    }
}

impl Default for Parser {
    fn default() -> Self { Self::new() }
}
```

### 4.7 Variable-Width Grid (grid.rs)

```rust
// grid.rs
//! Variable-width terminal grid — строки содержат TextRun спаны,
//! не фиксированные ячейки

use crate::{TermColor, TextAttrs};

/// Пиксельная позиция
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PixelPos {
    pub x: f32,
    pub y: f32,
}

/// Текстовый спан с однородными атрибутами
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    /// Текст (UTF-8)
    pub text: String,
    /// Цвет текста
    pub fg: TermColor,
    /// Цвет фона
    pub bg: TermColor,
    /// Атрибуты
    pub attrs: TextAttrs,
}

/// Строка терминала
#[derive(Debug, Clone)]
pub struct Line {
    /// Текстовые спаны
    pub runs: Vec<TextRun>,
    /// Строка была изменена с последнего рендера
    pub dirty: bool,
}

impl Line {
    pub fn new() -> Self {
        Self { runs: Vec::new(), dirty: true }
    }

    pub fn clear(&mut self) {
        self.runs.clear();
        self.dirty = true;
    }

    /// Получить весь текст строки
    pub fn text(&self) -> String {
        let mut s = String::new();
        for run in &self.runs {
            s.push_str(&run.text);
        }
        s
    }

    /// Длина строки в символах (для VT cursor positioning)
    pub fn char_count(&self) -> usize {
        self.runs.iter().map(|r| r.text.chars().count()).sum()
    }
}

impl Default for Line {
    fn default() -> Self { Self::new() }
}

/// Основная структура — хранит все строки + scrollback
pub struct Grid {
    /// Строки: [scrollback..., visible...]
    lines: Vec<Line>,
    /// Количество видимых строк (остальные — scrollback)
    visible_rows: usize,
    /// Максимальный scrollback
    max_scrollback: usize,
    /// Пиксельные размеры viewport
    pub width_px: u32,
    pub height_px: u32,

    // Cursor state
    /// Строка курсора (0-based, относительно visible area)
    pub cursor_row: usize,
    /// Колонка курсора (0-based, в символах — для VT совместимости)
    pub cursor_col: usize,
    pub cursor_visible: bool,
    pub cursor_style: CursorStyle,

    /// Saved cursor (DECSC/DECRC)
    saved_cursor: Option<(usize, usize)>,

    /// Scroll region (top, bottom) — 0-based, включительно
    scroll_top: usize,
    scroll_bottom: usize,

    /// Текущие атрибуты рисования
    pub current_fg: TermColor,
    pub current_bg: TermColor,
    pub current_attrs: TextAttrs,

    /// Alternate screen buffer
    alt_lines: Option<Vec<Line>>,
    alt_cursor: Option<(usize, usize)>,

    /// Режимы
    pub origin_mode: bool,
    pub auto_wrap: bool,
    pub bracketed_paste: bool,
    pub mouse_mode: MouseMode,
    pub cursor_keys_app: bool,

    /// Scrollback offset для просмотра (0 = live)
    pub scrollback_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorStyle { #[default] Block, Beam, Underline }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode { #[default] None, X10, ButtonEvent, AnyEvent, Sgr }

impl Grid {
    pub fn new(visible_rows: usize, max_scrollback: usize) -> Self {
        let mut lines = Vec::with_capacity(visible_rows);
        for _ in 0..visible_rows {
            lines.push(Line::new());
        }
        Self {
            lines,
            visible_rows,
            max_scrollback,
            width_px: 0,
            height_px: 0,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            cursor_style: CursorStyle::Block,
            saved_cursor: None,
            scroll_top: 0,
            scroll_bottom: visible_rows.saturating_sub(1),
            current_fg: TermColor::Default,
            current_bg: TermColor::Default,
            current_attrs: TextAttrs::empty(),
            alt_lines: None,
            alt_cursor: None,
            origin_mode: false,
            auto_wrap: true,
            bracketed_paste: false,
            mouse_mode: MouseMode::None,
            cursor_keys_app: false,
            scrollback_offset: 0,
        }
    }

    /// Количество строк в scrollback
    pub fn scrollback_len(&self) -> usize {
        self.lines.len().saturating_sub(self.visible_rows)
    }

    /// Индекс первой видимой строки
    fn visible_start(&self) -> usize {
        self.lines.len().saturating_sub(self.visible_rows)
    }

    /// Получить мутабельную ссылку на строку по visible row index
    fn line_mut(&mut self, row: usize) -> &mut Line {
        let idx = self.visible_start() + row;
        &mut self.lines[idx]
    }

    /// Получить ссылку на строку по visible row index
    fn line(&self, row: usize) -> &Line {
        let idx = self.visible_start() + row;
        &self.lines[idx]
    }

    /// Напечатать символ в текущей позиции курсора
    pub fn print(&mut self, c: char) {
        if self.cursor_row >= self.visible_rows {
            return;
        }
        let line = self.line_mut(self.cursor_row);

        // Попытаться добавить к последнему run если атрибуты совпадают
        if let Some(last) = line.runs.last_mut() {
            if last.fg == self.current_fg
                && last.bg == self.current_bg
                && last.attrs == self.current_attrs
            {
                // Проверяем, что курсор в конце строки
                let total_chars = line.char_count();
                if self.cursor_col >= total_chars.saturating_sub(last.text.chars().count())  {
                    last.text.push(c);
                    line.dirty = true;
                    self.cursor_col += 1;
                    return;
                }
            }
        }

        // Создаём новый run
        line.runs.push(TextRun {
            text: c.to_string(),
            fg: self.current_fg,
            bg: self.current_bg,
            attrs: self.current_attrs,
        });
        line.dirty = true;
        self.cursor_col += 1;
    }

    /// Новая строка (LF)
    pub fn linefeed(&mut self) {
        if self.cursor_row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor_row < self.visible_rows - 1 {
            self.cursor_row += 1;
        }
    }

    /// Carriage return
    pub fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    /// Scroll region up by N lines
    pub fn scroll_up(&mut self, n: usize) {
        for _ in 0..n {
            let remove_idx = self.visible_start() + self.scroll_top;
            if self.scroll_top == 0 {
                // Строка уходит в scrollback
                if self.scrollback_len() >= self.max_scrollback {
                    self.lines.remove(0); // Remove oldest scrollback
                }
                // Вставляем пустую строку в scroll_bottom
                let insert_idx = self.visible_start() + self.scroll_bottom;
                self.lines.insert(insert_idx + 1, Line::new());
            } else {
                self.lines.remove(remove_idx);
                let insert_idx = self.visible_start() + self.scroll_bottom;
                self.lines.insert(insert_idx, Line::new());
            }
        }
        // Mark all visible as dirty
        for row in self.scroll_top..=self.scroll_bottom {
            self.line_mut(row).dirty = true;
        }
    }

    /// Scroll region down by N lines
    pub fn scroll_down(&mut self, n: usize) {
        for _ in 0..n {
            let remove_idx = self.visible_start() + self.scroll_bottom;
            if remove_idx < self.lines.len() {
                self.lines.remove(remove_idx);
            }
            let insert_idx = self.visible_start() + self.scroll_top;
            self.lines.insert(insert_idx, Line::new());
        }
        for row in self.scroll_top..=self.scroll_bottom {
            self.line_mut(row).dirty = true;
        }
    }

    /// Erase display
    pub fn erase_display(&mut self, mode: super::parser::EraseMode) {
        use super::parser::EraseMode;
        match mode {
            EraseMode::ToEnd => {
                // Erase from cursor to end
                self.erase_line(EraseMode::ToEnd);
                for row in (self.cursor_row + 1)..self.visible_rows {
                    self.line_mut(row).clear();
                }
            }
            EraseMode::ToStart => {
                for row in 0..self.cursor_row {
                    self.line_mut(row).clear();
                }
                self.erase_line(EraseMode::ToStart);
            }
            EraseMode::All => {
                for row in 0..self.visible_rows {
                    self.line_mut(row).clear();
                }
            }
            EraseMode::Scrollback => {
                let start = self.visible_start();
                self.lines.drain(0..start);
            }
        }
    }

    /// Erase line
    pub fn erase_line(&mut self, mode: super::parser::EraseMode) {
        use super::parser::EraseMode;
        let line = self.line_mut(self.cursor_row);
        match mode {
            EraseMode::All => line.clear(),
            EraseMode::ToEnd => {
                // Truncate runs at cursor position
                let mut char_pos = 0;
                let cursor = self.cursor_col;
                let mut truncate_idx = line.runs.len();
                for (i, run) in line.runs.iter_mut().enumerate() {
                    let run_chars = run.text.chars().count();
                    if char_pos + run_chars > cursor {
                        // Trim this run
                        let trim_at = cursor - char_pos;
                        let new_text: String = run.text.chars().take(trim_at).collect();
                        run.text = new_text;
                        truncate_idx = if run.text.is_empty() { i } else { i + 1 };
                        break;
                    }
                    char_pos += run_chars;
                }
                line.runs.truncate(truncate_idx);
                line.dirty = true;
            }
            EraseMode::ToStart => {
                // Similar logic for erasing from start to cursor
                line.clear(); // Simplified
            }
            EraseMode::Scrollback => {} // N/A for line
        }
    }

    /// Enter alternate screen buffer
    pub fn enter_alt_screen(&mut self) {
        let mut alt = Vec::with_capacity(self.visible_rows);
        for _ in 0..self.visible_rows {
            alt.push(Line::new());
        }
        self.alt_lines = Some(std::mem::replace(
            &mut self.lines,
            alt,
        ));
        self.alt_cursor = Some((self.cursor_row, self.cursor_col));
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    /// Exit alternate screen buffer
    pub fn exit_alt_screen(&mut self) {
        if let Some(lines) = self.alt_lines.take() {
            self.lines = lines;
        }
        if let Some((row, col)) = self.alt_cursor.take() {
            self.cursor_row = row;
            self.cursor_col = col;
        }
        // Mark all dirty
        for row in 0..self.visible_rows {
            self.line_mut(row).dirty = true;
        }
    }

    /// Получить видимые строки для рендера
    pub fn visible_lines(&self) -> &[Line] {
        let start = self.visible_start().saturating_sub(self.scrollback_offset);
        let end = (start + self.visible_rows).min(self.lines.len());
        &self.lines[start..end]
    }

    /// Set visible rows (on resize)
    pub fn set_visible_rows(&mut self, rows: usize) {
        while self.lines.len() < self.visible_start() + rows {
            self.lines.push(Line::new());
        }
        self.visible_rows = rows;
        self.scroll_bottom = rows.saturating_sub(1);
        if self.cursor_row >= rows {
            self.cursor_row = rows.saturating_sub(1);
        }
    }

    /// Full reset
    pub fn reset(&mut self) {
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.cursor_visible = true;
        self.current_fg = TermColor::Default;
        self.current_bg = TermColor::Default;
        self.current_attrs = TextAttrs::empty();
        self.scroll_top = 0;
        self.scroll_bottom = self.visible_rows.saturating_sub(1);
        self.origin_mode = false;
        self.auto_wrap = true;
        self.bracketed_paste = false;
        self.mouse_mode = MouseMode::None;
        self.cursor_keys_app = false;
        for row in 0..self.visible_rows {
            self.line_mut(row).clear();
        }
    }
}
```

### 4.8 VT Emulator (emulator.rs)

```rust
// emulator.rs
use crate::grid::{CursorStyle, Grid, Line, MouseMode, PixelPos, TextRun};
use crate::parser::{Action, EraseMode, Parser, SgrAction};
use crate::{TermColor, TextAttrs};

/// Cursor state для рендерера
#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
    pub style: CursorStyle,
}

/// Snapshot для рендера — все данные нужные GPU
pub struct RenderSnapshot {
    pub lines: Vec<Line>,
    pub cursor: CursorState,
    pub title: String,
}

/// Трейт терминального эмулятора
pub trait TerminalEmulator: Send {
    /// Feed raw bytes from PTY
    fn process(&mut self, bytes: &[u8]);
    /// Resize — visible_rows определяется рендерером
    fn set_visible_rows(&mut self, rows: usize);
    /// Pixel size для рендерера
    fn set_pixel_size(&mut self, width: u32, height: u32);
    /// Snapshot для GPU рендера
    fn snapshot(&self) -> RenderSnapshot;
    /// Scrollback offset
    fn scrollback(&self) -> usize;
    fn set_scrollback(&mut self, offset: usize);
    /// Mouse mode
    fn mouse_mode(&self) -> MouseMode;
    /// Bracketed paste
    fn bracketed_paste(&self) -> bool;
    /// Application cursor keys
    fn cursor_keys_app(&self) -> bool;
    /// Window title
    fn title(&self) -> &str;
}

pub struct VtEmulator {
    parser: Parser,
    grid: Grid,
    title: String,
    /// Ответы для PTY (DSR, etc.)
    response_buf: Vec<u8>,
}

impl VtEmulator {
    pub fn new(width_px: u32, height_px: u32, scrollback: usize) -> Self {
        // Начальное кол-во строк — будет обновлено рендерером
        let rows = 24;
        let mut grid = Grid::new(rows, scrollback);
        grid.width_px = width_px;
        grid.height_px = height_px;
        Self {
            parser: Parser::new(),
            grid,
            title: String::new(),
            response_buf: Vec::new(),
        }
    }

    /// Получить и очистить буфер ответов (для записи в PTY)
    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.response_buf)
    }

    fn apply_action(&mut self, action: Action) {
        match action {
            Action::Print(c) => self.grid.print(c),
            Action::Bell => {} // Could trigger visual bell
            Action::Backspace => {
                self.grid.cursor_col = self.grid.cursor_col.saturating_sub(1);
            }
            Action::Tab => {
                let next = (self.grid.cursor_col / 8 + 1) * 8;
                self.grid.cursor_col = next;
            }
            Action::LineFeed => self.grid.linefeed(),
            Action::CarriageReturn => self.grid.carriage_return(),
            Action::CursorUp(n) => {
                self.grid.cursor_row = self.grid.cursor_row.saturating_sub(n as usize);
            }
            Action::CursorDown(n) => {
                let max = self.grid.scroll_bottom;
                self.grid.cursor_row = (self.grid.cursor_row + n as usize).min(max);
            }
            Action::CursorForward(n) => {
                self.grid.cursor_col += n as usize;
            }
            Action::CursorBack(n) => {
                self.grid.cursor_col = self.grid.cursor_col.saturating_sub(n as usize);
            }
            Action::CursorPosition { row, col } => {
                self.grid.cursor_row = (row as usize).saturating_sub(1);
                self.grid.cursor_col = (col as usize).saturating_sub(1);
            }
            Action::CursorColumn(col) => {
                self.grid.cursor_col = (col as usize).saturating_sub(1);
            }
            Action::EraseDisplay(mode) => self.grid.erase_display(mode),
            Action::EraseLine(mode) => self.grid.erase_line(mode),
            Action::InsertLines(n) => {
                for _ in 0..n {
                    self.grid.scroll_down(1);
                }
            }
            Action::DeleteLines(n) => {
                for _ in 0..n {
                    self.grid.scroll_up(1);
                }
            }
            Action::ScrollUp(n) => self.grid.scroll_up(n as usize),
            Action::ScrollDown(n) => self.grid.scroll_down(n as usize),
            Action::SetAttr(sgr) => self.apply_sgr(sgr),
            Action::SetScrollRegion { top, bottom } => {
                let t = (top as usize).saturating_sub(1);
                let b = if bottom == u16::MAX {
                    self.grid.visible_rows.saturating_sub(1)
                } else {
                    (bottom as usize).saturating_sub(1)
                };
                self.grid.scroll_top = t;
                self.grid.scroll_bottom = b;
                self.grid.cursor_row = 0;
                self.grid.cursor_col = 0;
            }
            Action::DecModeSet(mode) => self.set_dec_mode(mode, true),
            Action::DecModeReset(mode) => self.set_dec_mode(mode, false),
            Action::DeviceStatusReport(n) => {
                if n == 6 {
                    // Report cursor position
                    let response = format!(
                        "\x1b[{};{}R",
                        self.grid.cursor_row + 1,
                        self.grid.cursor_col + 1
                    );
                    self.response_buf.extend_from_slice(response.as_bytes());
                }
            }
            Action::SaveCursor => {
                self.grid.saved_cursor = Some((self.grid.cursor_row, self.grid.cursor_col));
            }
            Action::RestoreCursor => {
                if let Some((row, col)) = self.grid.saved_cursor {
                    self.grid.cursor_row = row;
                    self.grid.cursor_col = col;
                }
            }
            Action::ReverseIndex => {
                if self.grid.cursor_row == self.grid.scroll_top {
                    self.grid.scroll_down(1);
                } else {
                    self.grid.cursor_row = self.grid.cursor_row.saturating_sub(1);
                }
            }
            Action::FullReset => {
                self.grid.reset();
                self.title.clear();
            }
            Action::SetTitle(title) => {
                self.title = title;
            }
        }
    }

    fn apply_sgr(&mut self, sgr: SgrAction) {
        match sgr {
            SgrAction::Reset => {
                self.grid.current_fg = TermColor::Default;
                self.grid.current_bg = TermColor::Default;
                self.grid.current_attrs.reset();
            }
            SgrAction::SetAttr(flag) => self.grid.current_attrs.set(flag),
            SgrAction::ClearAttr(flag) => self.grid.current_attrs.clear(flag),
            SgrAction::Foreground(c) => self.grid.current_fg = c,
            SgrAction::Background(c) => self.grid.current_bg = c,
            SgrAction::DefaultForeground => self.grid.current_fg = TermColor::Default,
            SgrAction::DefaultBackground => self.grid.current_bg = TermColor::Default,
        }
    }

    fn set_dec_mode(&mut self, mode: u16, enable: bool) {
        match mode {
            1 => self.grid.cursor_keys_app = enable,
            25 => self.grid.cursor_visible = enable,
            47 | 1047 => {
                if enable { self.grid.enter_alt_screen(); }
                else { self.grid.exit_alt_screen(); }
            }
            1000 => self.grid.mouse_mode = if enable { MouseMode::X10 } else { MouseMode::None },
            1002 => self.grid.mouse_mode = if enable { MouseMode::ButtonEvent } else { MouseMode::None },
            1003 => self.grid.mouse_mode = if enable { MouseMode::AnyEvent } else { MouseMode::None },
            1006 => self.grid.mouse_mode = if enable { MouseMode::Sgr } else { MouseMode::None },
            1049 => {
                if enable {
                    self.grid.enter_alt_screen();
                    self.grid.erase_display(EraseMode::All);
                } else {
                    self.grid.exit_alt_screen();
                }
            }
            2004 => self.grid.bracketed_paste = enable,
            _ => {} // Unknown mode — ignore
        }
    }
}

impl TerminalEmulator for VtEmulator {
    fn process(&mut self, bytes: &[u8]) {
        let mut actions = Vec::new();
        self.parser.advance(bytes, |action| actions.push(action));
        for action in actions {
            self.apply_action(action);
        }
    }

    fn set_visible_rows(&mut self, rows: usize) {
        self.grid.set_visible_rows(rows);
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.grid.width_px = width;
        self.grid.height_px = height;
    }

    fn snapshot(&self) -> RenderSnapshot {
        RenderSnapshot {
            lines: self.grid.visible_lines().to_vec(),
            cursor: CursorState {
                row: self.grid.cursor_row,
                col: self.grid.cursor_col,
                visible: self.grid.cursor_visible,
                style: self.grid.cursor_style,
            },
            title: self.title.clone(),
        }
    }

    fn scrollback(&self) -> usize { self.grid.scrollback_offset }
    fn set_scrollback(&mut self, offset: usize) {
        self.grid.scrollback_offset = offset.min(self.grid.scrollback_len());
    }

    fn mouse_mode(&self) -> MouseMode { self.grid.mouse_mode }
    fn bracketed_paste(&self) -> bool { self.grid.bracketed_paste }
    fn cursor_keys_app(&self) -> bool { self.grid.cursor_keys_app }
    fn title(&self) -> &str { &self.title }
}
```

---

## 5. term_gpu: GPU Рендерер

> Подробная спецификация в отдельном файле: [gpu-renderer-spec.md](gpu-renderer-spec.md)

### 5.1 Структура файлов

```
crates/term_gpu/src/
├── lib.rs              # Публичный API
├── renderer.rs         # GpuRenderer — главный рендерер
├── surface.rs          # Surface management (wgpu::Surface)
├── pipeline.rs         # Render pipelines (text + prim)
├── atlas.rs            # GlyphAtlas + ShelfPacker + LRU Cache
├── text.rs             # cosmic-text интеграция, text shaping
├── instances.rs        # Instance buffers (GlyphInstance, PrimInstance)
├── color.rs            # TermColor → RGBA конвертация для GPU
└── shaders/
    ├── text.wgsl       # Шейдер текста
    └── prim.wgsl       # Шейдер примитивов
```

### 5.2 WGSL шейдеры

#### text.wgsl
```wgsl
struct Uniforms {
    screen_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

struct GlyphInput {
    @location(0) pos: vec2<f32>,       // pixel position
    @location(1) size: vec2<f32>,      // glyph size in pixels
    @location(2) uv_min: vec2<f32>,    // atlas UV min
    @location(3) uv_max: vec2<f32>,    // atlas UV max
    @location(4) color: vec4<f32>,     // text color RGBA
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

// 6 vertices per quad (2 triangles)
const QUAD: array<vec2<f32>, 6> = array(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, g: GlyphInput) -> VsOut {
    let q = QUAD[vi];
    let px = g.pos + q * g.size;
    let ndc = (px / uniforms.screen_size) * 2.0 - 1.0;

    var out: VsOut;
    out.pos = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = mix(g.uv_min, g.uv_max, q);
    out.color = g.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let alpha = textureSample(atlas_tex, atlas_samp, in.uv).r;
    return vec4(in.color.rgb, in.color.a * alpha);
}
```

#### prim.wgsl
```wgsl
struct Uniforms {
    screen_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct RectInput {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

const QUAD: array<vec2<f32>, 6> = array(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, r: RectInput) -> VsOut {
    let q = QUAD[vi];
    let px = r.pos + q * r.size;
    let ndc = (px / uniforms.screen_size) * 2.0 - 1.0;

    var out: VsOut;
    out.pos = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.color = r.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
```

### 5.3 Instance structures (repr(C))

```rust
// instances.rs

/// Instance data для одного глифа
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GlyphInstance {
    pub pos: [f32; 2],       // pixel position
    pub size: [f32; 2],      // glyph size
    pub uv_min: [f32; 2],    // atlas UV min
    pub uv_max: [f32; 2],    // atlas UV max
    pub color: [f32; 4],     // RGBA
}

/// Instance data для примитива (bg rect, cursor, selection)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PrimInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
}

// Вместо bytemuck — свой unsafe cast
impl GlyphInstance {
    pub fn as_bytes(slice: &[Self]) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                slice.as_ptr() as *const u8,
                slice.len() * std::mem::size_of::<Self>(),
            )
        }
    }
}

impl PrimInstance {
    pub fn as_bytes(slice: &[Self]) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                slice.as_ptr() as *const u8,
                slice.len() * std::mem::size_of::<Self>(),
            )
        }
    }
}
```

### 5.4 Glyph Atlas (atlas.rs)

```rust
// atlas.rs — собственный bin-packer + LRU cache

use cosmic_text::CacheKey;
use std::collections::HashMap;

const ATLAS_SIZE: u32 = 2048;
const GLYPH_PAD: u32 = 1;

/// Shelf-based bin packer
pub struct ShelfPacker {
    width: u32,
    height: u32,
    shelf_y: u32,
    shelf_height: u32,
    cursor_x: u32,
}

impl ShelfPacker {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, shelf_y: 0, shelf_height: 0, cursor_x: 0 }
    }

    pub fn pack(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let w = w + GLYPH_PAD * 2;
        let h = h + GLYPH_PAD * 2;
        if w > self.width { return None; }
        if self.cursor_x + w > self.width {
            self.shelf_y += self.shelf_height;
            self.cursor_x = 0;
            self.shelf_height = 0;
        }
        if self.shelf_y + h > self.height { return None; }
        let pos = (self.cursor_x + GLYPH_PAD, self.shelf_y + GLYPH_PAD);
        self.cursor_x += w;
        self.shelf_height = self.shelf_height.max(h);
        Some(pos)
    }

    pub fn reset(&mut self) {
        self.shelf_y = 0;
        self.shelf_height = 0;
        self.cursor_x = 0;
    }
}

/// Cached glyph info
#[derive(Debug, Clone, Copy)]
pub struct CachedGlyph {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub offset_x: f32,
    pub offset_y: f32,
    pub width: f32,
    pub height: f32,
}

/// LRU node
struct LruNode {
    key: CacheKey,
    glyph: CachedGlyph,
    prev: usize,
    next: usize,
}

const NIL: usize = usize::MAX;

/// LRU cache — intrusive doubly-linked list + HashMap
pub struct GlyphLru {
    nodes: Vec<LruNode>,
    map: HashMap<CacheKey, usize>,
    head: usize,   // MRU
    tail: usize,   // LRU
    len: usize,
    cap: usize,
}

impl GlyphLru {
    pub fn new(capacity: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
            map: HashMap::with_capacity(capacity),
            head: NIL,
            tail: NIL,
            len: 0,
            cap: capacity,
        }
    }

    pub fn get(&mut self, key: &CacheKey) -> Option<CachedGlyph> {
        let &idx = self.map.get(key)?;
        self.touch(idx);
        Some(self.nodes[idx].glyph)
    }

    pub fn insert(&mut self, key: CacheKey, glyph: CachedGlyph) {
        if let Some(&idx) = self.map.get(&key) {
            self.nodes[idx].glyph = glyph;
            self.touch(idx);
            return;
        }
        if self.len >= self.cap {
            self.evict_lru();
        }
        let idx = self.nodes.len();
        self.nodes.push(LruNode { key, glyph, prev: NIL, next: self.head });
        if self.head != NIL { self.nodes[self.head].prev = idx; }
        self.head = idx;
        if self.tail == NIL { self.tail = idx; }
        self.map.insert(key, idx);
        self.len += 1;
    }

    fn touch(&mut self, idx: usize) {
        if self.head == idx { return; }
        self.detach(idx);
        self.nodes[idx].prev = NIL;
        self.nodes[idx].next = self.head;
        if self.head != NIL { self.nodes[self.head].prev = idx; }
        self.head = idx;
    }

    fn detach(&mut self, idx: usize) {
        let prev = self.nodes[idx].prev;
        let next = self.nodes[idx].next;
        if prev != NIL { self.nodes[prev].next = next; } else { self.head = next; }
        if next != NIL { self.nodes[next].prev = prev; } else { self.tail = prev; }
    }

    fn evict_lru(&mut self) {
        if self.tail == NIL { return; }
        let idx = self.tail;
        self.detach(idx);
        self.map.remove(&self.nodes[idx].key);
        self.len -= 1;
        // Note: node stays in Vec (tombstone), could compact periodically
    }
}

/// Main atlas struct
pub struct GlyphAtlas {
    packer: ShelfPacker,
    lru: GlyphLru,
    cpu_data: Vec<u8>,       // R8 pixel data
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    dirty: bool,
}

impl GlyphAtlas {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph_atlas"),
            size: wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&Default::default());

        Self {
            packer: ShelfPacker::new(ATLAS_SIZE, ATLAS_SIZE),
            lru: GlyphLru::new(4096),
            cpu_data: vec![0u8; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            texture,
            texture_view,
            dirty: false,
        }
    }

    pub fn get_or_insert(
        &mut self,
        key: CacheKey,
        rasterize: impl FnOnce() -> Option<RasterizedGlyph>,
    ) -> Option<CachedGlyph> {
        if let Some(cached) = self.lru.get(&key) {
            return Some(cached);
        }
        let raster = rasterize()?;
        let (x, y) = self.packer.pack(raster.width, raster.height)?;

        // Copy glyph bitmap into CPU buffer
        for row in 0..raster.height {
            let src_off = (row * raster.width) as usize;
            let dst_off = ((y + row) * ATLAS_SIZE + x) as usize;
            self.cpu_data[dst_off..dst_off + raster.width as usize]
                .copy_from_slice(&raster.data[src_off..src_off + raster.width as usize]);
        }
        self.dirty = true;

        let cached = CachedGlyph {
            uv_min: [x as f32 / ATLAS_SIZE as f32, y as f32 / ATLAS_SIZE as f32],
            uv_max: [(x + raster.width) as f32 / ATLAS_SIZE as f32, (y + raster.height) as f32 / ATLAS_SIZE as f32],
            offset_x: raster.left as f32,
            offset_y: raster.top as f32,
            width: raster.width as f32,
            height: raster.height as f32,
        };
        self.lru.insert(key, cached);
        Some(cached)
    }

    pub fn upload(&mut self, queue: &wgpu::Queue) {
        if !self.dirty { return; }
        queue.write_texture(
            self.texture.as_image_copy(),
            &self.cpu_data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_SIZE),
                rows_per_image: Some(ATLAS_SIZE),
            },
            wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
        );
        self.dirty = false;
    }

    pub fn view(&self) -> &wgpu::TextureView { &self.texture_view }
}

/// Rasterized glyph data (from cosmic-text/swash)
pub struct RasterizedGlyph {
    pub data: Vec<u8>,   // Alpha values
    pub width: u32,
    pub height: u32,
    pub left: i32,       // Bearing X
    pub top: i32,        // Bearing Y
}
```

### 5.5 Render pass порядок

1. **Clear** — wgpu::LoadOp::Clear (фоновый цвет терминала)
2. **Background rects** — PrimInstance для каждого TextRun с bg != Default
3. **Glyphs** — GlyphInstance для каждого глифа из shaped text
4. **Cursor** — PrimInstance для курсора
5. **Selection overlay** — PrimInstance с полупрозрачным цветом
6. **Present** — output.present()

---

## 6. term_layout: BSP Panel Manager

### 6.1 Структура

```rust
// lib.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split { Horizontal, Vertical }

pub enum Node {
    Leaf { id: PanelId, bounds: Rect },
    Branch { split: Split, ratio: f32, bounds: Rect, left: Box<Node>, right: Box<Node> },
}

pub struct PanelTree {
    root: Option<Node>,
    next_id: u64,
    pub focus: PanelId,
}

impl PanelTree {
    pub fn new(w: f32, h: f32) -> Self { ... }
    pub fn split(&mut self, target: PanelId, split: Split, ratio: f32) -> PanelId { ... }
    pub fn close(&mut self, target: PanelId) { ... }
    pub fn resize(&mut self, w: f32, h: f32) { ... }
    pub fn hit_test(&self, x: f32, y: f32) -> Option<PanelId> { ... }
    pub fn panels(&self) -> Vec<(PanelId, Rect)> { ... }
}
```

---

## 7. Интеграция с текущим кодом

### Удаляемые файлы

| Файл | Причина |
|------|---------|
| `src/ui/terminal.rs` | Заменяется GPU рендерером |
| `src/ui/render.rs` | Заменяется GPU рендерером |
| `src/ui/layout.rs` | Заменяется term_layout |
| `src/ui/terminal_guard.rs` | crossterm setup — заменяется winit |
| `src/ui/events.rs` | EventHandler через crossterm → winit |
| `src/ui/theme.rs` | Цвета → AnsiPalette в term_core |
| `src/ui/header.rs` | Будет перенесён в GPU рендер |
| `src/ui/footer.rs` | Будет перенесён в GPU рендер |
| `src/pty/emulator/alacritty_impl.rs` | Заменяется term_core::VtEmulator |

### Изменяемые файлы

| Файл | Изменения |
|------|-----------|
| `src/pty/emulator/mod.rs` | `pub use term_core::emulator::TerminalEmulator;` — трейт из term_core |
| `src/pty/session.rs` | `emulator::create()` → `term_core::create_emulator()` |
| `src/pty/handle.rs` | Адаптировать под новый трейт (set_visible_rows вместо set_size) |
| `src/ui/runtime.rs` | Главная перестройка: ratatui loop → winit ApplicationHandler |
| `src/ui/app.rs` | Адаптировать под новый трейт TerminalEmulator |
| `src/ui/input.rs` | classify_key адаптировать под winit KeyEvent |
| `src/ui/selection.rs` | Pixel-based selection вместо grid-based |
| `Cargo.toml` | Описано в секции 3 |

### Файлы без изменений

Всё в `src/proxy/`, `src/config/`, `src/metrics/`, `src/ipc/`, `src/shim/`, `src/args/`, `src/clipboard/`, `src/error/`, `src/shutdown/`.

---

## 8. Roadmap

### Фаза 1: term_core (2 недели)
**Файлы:** `crates/term_core/src/{lib,parser,color,attrs,grid,emulator}.rs`
**Deliverable:** VT парсер проходит тесты с выводом Claude Code
**Тесты:** Захватить реальный вывод Claude Code, воспроизвести через парсер

### Фаза 2: term_gpu — base (2 недели)
**Файлы:** `crates/term_gpu/src/{lib,renderer,surface,pipeline,instances,shaders/}.rs`
**Deliverable:** Окно с цветным текстом через wgpu

### Фаза 3: term_gpu — text (2 недели)
**Файлы:** `crates/term_gpu/src/{atlas,text,color}.rs`
**Deliverable:** Variable-width текст с cosmic-text, glyph atlas, LRU cache

### Фаза 4: term_layout (1 неделя)
**Файлы:** `crates/term_layout/src/lib.rs`
**Deliverable:** BSP панели, split/close/resize

### Фаза 5: Integration (2 недели)
**Файлы:** `src/ui/runtime.rs`, `src/pty/emulator/mod.rs`, `src/pty/session.rs`, `src/pty/handle.rs`
**Deliverable:** AnyClaude работает с GPU терминалом, Claude Code отображается корректно

### Фаза 6: Polish (1 неделя)
**Deliverable:** Selection, clipboard, scrollback, font fallback, performance tuning

**Итого: ~10 недель**
