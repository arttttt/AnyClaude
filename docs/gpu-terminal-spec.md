# GPU Terminal Specification — AnyClaude

## 1. Overview

### Goal
Replace the textual TUI renderer (ratatui + alacritty_terminal) with a custom GPU renderer that supports variable-width fonts and a panel system.

### Current stack
```
PTY (portable-pty) → alacritty_terminal (VT parser) → TermCell grid → ratatui → stdout
```

**Current stack limitations:**
- Monospace-only rendering via ratatui
- Fixed cell grid (row/col), no pixel positioning
- No panel support (layout: header + body + footer, single terminal panel)
- No GPU acceleration, rendering via escape sequences to stdout
- Dependency on alacritty_terminal — a heavy full VT emulator, excessive for Claude Code

### Target stack
```
PTY (portable-pty) → term_core (minimal VT parser) → Cell grid → wgpu → GPU
```

**Advantages:**
- Variable-width fonts with pixel positioning
- GPU-accelerated rendering (Metal on macOS, Vulkan on Linux)
- Panel system (BSP tree)
- Minimal VT parser — only what Claude Code (ink-based TUI) needs
- Minimal dependencies

---

## 2. Dependencies

### External (only 3 crates)

| Crate | Version | Purpose | Why we cannot write it ourselves |
|-------|---------|---------|----------------------------------|
| `wgpu` | 24 | GPU abstraction (Metal/Vulkan/DX12) | Thousands of lines of platform code, active driver support |
| `winit` | 0.30 | Windows, events, DPI | Platform-specific windowing (Cocoa, Wayland, X11) |
| `cosmic-text` | 0.14 | Text shaping for variable-width | Unicode shaping (HarfBuzz-level complexity), BiDi, font fallback |

### Removed dependencies

| Crate | Reason for removal |
|-------|--------------------|
| `ratatui` | Replaced by GPU renderer |
| `alacritty_terminal` | Replaced by term_core |
| `crossterm` | Replaced by winit (events) |
| `signal-hook` | Folded into winit event loop |

### Unchanged

portable-pty, tokio, axum, reqwest, clap, serde/serde_json/toml, dirs, parking_lot, arboard, anyhow, tempfile, uuid, term_input (adapted).

---

## 3. Crate structure

```
Cargo.toml (workspace)
├── src/                    # anyclaude — main app
├── crates/
│   ├── term_input/         # [existing] input handling from /dev/tty
│   ├── term_core/          # [NEW] minimal VT parser + variable-width grid
│   ├── term_gpu/           # [NEW] wgpu renderer, glyph atlas, shaders
│   └── term_layout/        # [NEW] BSP panel manager
```

### Dependencies between crates

```
term_core         (0 external deps, std only)
    ↑
term_gpu          → wgpu, cosmic-text, term_core
    ↑
term_layout       → term_gpu, term_core
    ↑
anyclaude         → term_layout, term_gpu, term_core, term_input
```

### Crate Cargo.toml files

#### crates/term_core/Cargo.toml
```toml
[package]
name = "term_core"
version = "0.1.0"
edition = "2021"

[lints]
workspace = true

# Zero external dependencies — std only
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
pollster = "0.4"        # block_on for async wgpu init
futures = "0.3"         # abortable handle for the momentum timer
futures-timer = "3"     # Delay for momentum ticks (see §5.7)
glam = "0.30"           # Vec2 for scroll velocity
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

#### Updated root Cargo.toml (changes)
```toml
[workspace]
members = [".", "crates/term_input", "crates/term_core", "crates/term_gpu", "crates/term_layout"]

[dependencies]
# REMOVE:
# ratatui = "0.30"
# alacritty_terminal = "0.25"
# crossterm = "0.29"
# signal-hook = "0.4"

# ADD:
term_core = { path = "crates/term_core" }
term_gpu = { path = "crates/term_gpu" }
term_layout = { path = "crates/term_layout" }

# KEEP UNCHANGED:
# portable-pty, tokio, axum, reqwest, clap, serde, etc.
```

---

## 4. term_core: Minimal VT parser

### 4.1 File structure

```
crates/term_core/src/
├── lib.rs          # Public API, re-exports
├── parser.rs       # Paul Williams VT state machine (hand-rolled, ~500 LoC)
├── grid.rs         # Fixed-cell grid (Cell, Row, Grid; alacritty-style)
├── emulator.rs     # VtEmulator — TerminalEmulator trait implementation
├── color.rs        # TermColor, ANSI palette
└── attrs.rs        # CellFlags (bold, italic, underline, …)
```

**Why fixed-cell instead of variable-width spans?** ink (Claude Code's
TUI framework) assumes a monospace grid for cursor positioning — `CUP
row 5 col 10` must address a definite cell, not a pixel range.
Variable-width rendering happens in `term_gpu`, which shapes each row
with cosmic-text and lays out glyphs by their actual advance widths.
Logical model is fixed; visual model is variable. See
[`docs/analysis/warp-vt-parser-research.md`](analysis/warp-vt-parser-research.md)
§1 for Warp's identical choice.

### 4.2 Supported escape sequences

Only what Claude Code (ink-based TUI) uses:

#### CSI sequences (`ESC [`)

Priority tags follow the research recommendations
([`warp-vt-parser-research.md`](analysis/warp-vt-parser-research.md) §3):
**P0** required for Claude Code to render correctly, **P1** modern UX
with low complexity, **P2** robustness improvements. Lines without a
tag were in the original spec before the research.

| Sequence | Code | Priority | Purpose |
|----------|-----|----------|-----------|
| ICH | `ESC[{n}@` | **P0** | Insert blank chars |
| CUU | `ESC[{n}A` | | Cursor Up |
| CUD | `ESC[{n}B` | | Cursor Down |
| CUF | `ESC[{n}C` | | Cursor Forward |
| CUB | `ESC[{n}D` | | Cursor Back |
| CNL | `ESC[{n}E` | **P0** | Cursor next line (CUD + col 1) |
| CPL | `ESC[{n}F` | **P0** | Cursor previous line (CUU + col 1) |
| CHA | `ESC[{n}G` | | Cursor Horizontal Absolute |
| CUP | `ESC[{r};{c}H` | | Cursor Position |
| CHT | `ESC[{n}I` | **P2** | Cursor forward tabs |
| ED | `ESC[{n}J` | | Erase Display (0=to end, 1=to start, 2=all, 3=scrollback) |
| EL | `ESC[{n}K` | | Erase Line (0=to end, 1=to start, 2=all) |
| IL | `ESC[{n}L` | | Insert Lines |
| DL | `ESC[{n}M` | | Delete Lines |
| DCH | `ESC[{n}P` | **P0** | Delete chars at cursor |
| SU | `ESC[{n}S` | | Scroll Up |
| SD | `ESC[{n}T` | | Scroll Down |
| ECH | `ESC[{n}X` | **P0** | Erase chars at cursor (no cursor move) |
| CBT | `ESC[{n}Z` | **P2** | Cursor backward tabs |
| HPA | `ESC[{n}\`` | **P2** | Horizontal position absolute (same as CHA) |
| REP | `ESC[{n}b` | **P0** | Repeat last character N times |
| DA | `ESC[c` | **P0** | Device attributes — emulator must reply `ESC[?6c` |
| VPA | `ESC[{n}d` | **P0** | Vertical position absolute |
| HVP | `ESC[{r};{c}f` | | Horizontal-vertical position (same as CUP) |
| TBC | `ESC[{n}g` | **P2** | Tab clear |
| SGR | `ESC[{...}m` | | Set Graphics Rendition (colors, styles) |
| DSR | `ESC[{n}n` | | Device Status Report (n=6 → report cursor pos) |
| DECRQM | `ESC[?{n}p` | **P1** | Request DEC mode state |
| DECSCUSR | `ESC[{n} q` | **P1** | Set cursor style (1-2 block, 3-4 underline, 5-6 beam) |
| DECSTBM | `ESC[{t};{b}r` | | Set Scroll Region |
| SCOSC | `ESC[s` | **P2** | Save cursor (SCO variant) |
| DECSET | `ESC[?{n}h` | | Set DEC Private Mode |
| DECRST | `ESC[?{n}l` | | Reset DEC Private Mode |
| SCORC | `ESC[u` | **P2** | Restore cursor (SCO variant) |

#### SGR parameters (`ESC[{...}m`)
| Code | Priority | Meaning |
|-----|----------|----------|
| 0 | | Reset |
| 1 | | Bold |
| 2 | | Faint |
| 3 | | Italic |
| 4 | | Underline |
| 4;2 | **P2** | Double underline |
| 4;0 | **P2** | Cancel underline |
| 5 | **P2** | Blink (slow) |
| 6 | **P2** | Blink (fast) |
| 7 | | Inverse |
| 8 | **P2** | Hidden |
| 9 | | Strikethrough |
| 21 | **P2** | Cancel bold (different from 22) |
| 22 | | Normal intensity |
| 23 | | Not italic |
| 24 | | Not underline |
| 27 | | Not inverse |
| 28 | **P2** | Cancel hidden |
| 29 | | Not strikethrough |
| 30-37 | | Foreground (standard) |
| 38;5;{n} | | Foreground (256-color) |
| 38;2;{r};{g};{b} | | Foreground (truecolor) |
| 39 | | Default foreground |
| 40-47 | | Background (standard) |
| 48;5;{n} | | Background (256-color) |
| 48;2;{r};{g};{b} | | Background (truecolor) |
| 49 | | Default background |
| 90-97 | | Foreground (bright) |
| 100-107 | | Background (bright) |

#### DEC Private Modes (`ESC[?{n}h/l`)
| Code | Mode | Priority | Purpose |
|-----|-------|----------|-----------|
| 1 | DECCKM | | Application cursor keys |
| 6 | DECOM | **P0** | Origin mode — CUP within scrolling region |
| 7 | DECAWM | **P0** | Autowrap at right margin |
| 12 | | **P1** | Blinking cursor |
| 25 | DECTCEM | | Cursor visible/hidden |
| 47 | Alt screen (save) | | Alternate screen buffer |
| 1000 | X10 mouse | | Basic mouse tracking |
| 1002 | Button event | | Mouse button events |
| 1003 | Any event | | All mouse events |
| 1004 | Focus events | **P1** | Emit `CSI I` on focus, `CSI O` on blur |
| 1006 | SGR mouse | | SGR mouse encoding |
| 1007 | Alt scroll | **P2** | Wheel produces arrow keys in alt screen |
| 1049 | Alt screen (save+clear) | | Alternate screen buffer + clear |
| 2004 | Bracketed paste | | Bracketed paste mode |
| 2026 | Sync output | **P2** | Buffer output frames atomically (150 ms / 2 MiB cap) |

#### OSC sequences (`ESC ]`)
| Sequence | Priority | Purpose |
|----------|----------|-----------|
| `OSC 0;{title} ST` | | Set window title (and icon name) |
| `OSC 2;{title} ST` | | Set window title only |
| `OSC 7;file://host/{path} ST` | **P1** | Notify shell CWD |
| `OSC 8;params;url ST` | **P2** | Hyperlink (modern apps; Warp ignores, we may want) |
| `OSC 133;A ST` | **P1** | Prompt start marker (FinalTerm/shell integration) |
| `OSC 133;B ST` | **P1** | Prompt end marker |
| `OSC 133;P;k=v ST` | **P1** | Prompt param payload |

OSC strings are terminated by `ST` (`ESC \`, two bytes) or `BEL`
(`0x07`, one byte). The parser must accept both.

#### Simple escape sequences
| Sequence | Priority | Purpose |
|----------|----------|-----------|
| `ESC 7` | | Save cursor (DECSC) |
| `ESC 8` | | Restore cursor (DECRC) |
| `ESC D` | **P2** | Index (IND) — move cursor down, scroll if at bottom |
| `ESC E` | **P2** | Next line (NEL) — equivalent to CR + LF |
| `ESC M` | | Reverse index — move up, scroll down if at top |
| `ESC =` | **P2** | Application keypad mode (DECPAM) |
| `ESC >` | **P2** | Numeric keypad mode (DECPNM) |
| `ESC c` | | Full reset (RIS) |

#### Control characters
| Byte | Priority | Purpose |
|------|----------|-----------|
| 0x07 (BEL) | | Bell |
| 0x08 (BS) | | Backspace |
| 0x09 (HT) | | Tab (fixed at 8 columns) |
| 0x0A (LF) | | Line feed |
| 0x0B (VT) | **P2** | Vertical tab — treat as LF |
| 0x0C (FF) | **P2** | Form feed — treat as LF |
| 0x0D (CR) | | Carriage return |
| 0x1A (SUB) | **P2** | Substitute — abort current escape sequence |

### 4.3 What is NOT supported

Either out of scope for our use case (Claude Code wrapping) or
explicitly skipped per the [Warp VT research](analysis/warp-vt-parser-research.md) §4.

- **DCS, SOS, PM, APC** sequences — eaten without dispatch (state
  machine consumes them so they don't corrupt subsequent input)
- **Character sets G2/G3** (`ESC * / +`). G0/G1 + the line-drawing
  table (`ESC ( 0`) is P3 — add only if observed in real traces.
- **VT52 compatibility mode**
- **Double width/height lines** (DECDWL, DECDHL)
- **Custom tab stops** (HTS sets, TBC clears — P2; otherwise fixed
  tab = 8 is used)
- **Printer control** (MC)
- **Soft fonts** (DECDLD)
- **Rectangular area operations** (DECRARA, DECCRA)
- **Macro sequences**
- **iTerm inline images** (OSC 1337) and **Kitty image protocol**
  (APC). Claude Code does not emit images.
- **Sixel graphics**
- **Tmux control mode** (Warp's `TmuxControlModeParser`) — we wrap
  Claude Code, not tmux
- **OSC 4 palette manipulation** and **OSC 10/11/12 dynamic colors
  with `?` query** — Warp uses these for theme integration; Claude
  Code does not change palette
- **Warp-specific OSCs** (`OSC 9277..9280`, `OSC 781378`) — Warp-only
- **Kitty keyboard protocol** (`CSI u`) — P3, add only if Claude Code
  uses modifiers beyond Alt-prefixed
- **OSC 52 clipboard**, **OSC 10/11/12 color queries** — niche

### 4.4 Public API (lib.rs)

```rust
pub mod parser;
pub mod grid;
pub mod emulator;
pub mod color;
pub mod attrs;

pub use color::{AnsiPalette, TermColor};
pub use attrs::CellFlags;
pub use grid::{Cell, Grid, Row};
pub use emulator::{CursorState, CursorStyle, MouseMode, RenderSnapshot, TerminalEmulator};
pub use parser::{Action, EraseMode, Parser, SgrAction};

/// Create a terminal emulator with the given visible grid size and
/// scrollback line cap. `cols` and `rows` are in cells (variable-width
/// rendering happens in `term_gpu`, the logical grid is fixed-cell).
pub fn create_emulator(cols: usize, rows: usize, scrollback: usize) -> Box<dyn TerminalEmulator> {
    Box::new(emulator::VtEmulator::new(cols, rows, scrollback))
}
```

### 4.5 Colors and attributes (color.rs, attrs.rs)

```rust
// color.rs
/// Terminal color — compatible with the current TermColor from emulator/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TermColor {
    #[default]
    Default,
    Indexed(u8),       // 0-255
    Rgb(u8, u8, u8),   // True color
}

impl TermColor {
    /// Standard ANSI palette (16 colors)
    pub const BLACK: Self = Self::Indexed(0);
    pub const RED: Self = Self::Indexed(1);
    pub const GREEN: Self = Self::Indexed(2);
    pub const YELLOW: Self = Self::Indexed(3);
    pub const BLUE: Self = Self::Indexed(4);
    pub const MAGENTA: Self = Self::Indexed(5);
    pub const CYAN: Self = Self::Indexed(6);
    pub const WHITE: Self = Self::Indexed(7);

    /// Convert to RGBA f32 for GPU
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
/// Text attributes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CellFlags {
    bits: u8,
}

impl CellFlags {
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
//! Minimal VT/ANSI parser — state machine for Claude Code

use crate::{TermColor, CellFlags};

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

/// Parser actions — dispatched to Grid/Emulator
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
    SetAttr(u8),       // CellFlags flag to set
    ClearAttr(u8),     // CellFlags flag to clear
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

    /// Parses bytes, invokes callback for each action.
    /// Zero-allocation for standard sequences.
    pub fn advance<F: FnMut(Action)>(&mut self, input: &[u8], mut emit: F) {
        for &byte in input {
            self.feed(byte, &mut emit);
        }
    }

    fn feed<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        // ESC and C0 are handled from any state (except OSC for ESC)
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
                1 => emit(Action::SetAttr(SgrAction::SetAttr(CellFlags::BOLD))),
                2 => emit(Action::SetAttr(SgrAction::SetAttr(CellFlags::FAINT))),
                3 => emit(Action::SetAttr(SgrAction::SetAttr(CellFlags::ITALIC))),
                4 => emit(Action::SetAttr(SgrAction::SetAttr(CellFlags::UNDERLINE))),
                5 => emit(Action::SetAttr(SgrAction::SetAttr(CellFlags::BLINK))),
                7 => emit(Action::SetAttr(SgrAction::SetAttr(CellFlags::INVERSE))),
                9 => emit(Action::SetAttr(SgrAction::SetAttr(CellFlags::STRIKE))),
                22 => {
                    emit(Action::SetAttr(SgrAction::ClearAttr(CellFlags::BOLD)));
                    emit(Action::SetAttr(SgrAction::ClearAttr(CellFlags::FAINT)));
                }
                23 => emit(Action::SetAttr(SgrAction::ClearAttr(CellFlags::ITALIC))),
                24 => emit(Action::SetAttr(SgrAction::ClearAttr(CellFlags::UNDERLINE))),
                27 => emit(Action::SetAttr(SgrAction::ClearAttr(CellFlags::INVERSE))),
                29 => emit(Action::SetAttr(SgrAction::ClearAttr(CellFlags::STRIKE))),
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

### 4.7 Fixed-Cell Grid (grid.rs)

> Adapted from `alacritty_terminal` (Apache-2.0) — same model Warp uses
> (see [`analysis/warp-vt-parser-research.md`](analysis/warp-vt-parser-research.md) §1).
> Cell holds a single grapheme cluster; wide characters use a `WIDE_CHAR`
> + `WIDE_CHAR_SPACER` flag pair so column arithmetic stays simple.
> Variable-width *rendering* happens in `term_gpu` — `term_core` is
> logically monospace-grid for VT correctness.

```rust
// grid.rs
//! Fixed-cell terminal grid.

use crate::{CellFlags, TermColor};

/// One grid cell. ~24 bytes target; we use a Box for the rare
/// per-cell metadata (combining marks, prompt markers) to keep the
/// hot path compact. For Claude Code we don't expect huge cell
/// volumes, so the packing isn't load-bearing — readability wins.
#[derive(Debug, Clone)]
pub struct Cell {
    /// Primary character. For wide characters this is on the left half;
    /// the right half is a spacer cell with `WIDE_CHAR_SPACER`.
    pub c: char,
    pub fg: TermColor,
    pub bg: TermColor,
    pub flags: CellFlags,
    /// Combining marks and other per-cell extras. `None` in the common case.
    pub extra: Option<Box<CellExtra>>,
}

impl Cell {
    pub const fn space() -> Self {
        Self {
            c: ' ',
            fg: TermColor::Default,
            bg: TermColor::Default,
            flags: CellFlags::empty(),
            extra: None,
        }
    }

    pub fn reset(&mut self) {
        *self = Cell::space();
    }

    /// Push a zero-width / combining codepoint onto this cell. Bounded by
    /// `MAX_ZEROWIDTH_BYTES` to avoid pathological input.
    pub fn push_zerowidth(&mut self, c: char) {
        let extra = self.extra.get_or_insert_with(Box::default);
        extra.push_zerowidth(c);
    }

    /// URL hyperlink target from OSC 8, if any.
    pub fn hyperlink(&self) -> Option<&str> {
        self.extra.as_ref().and_then(|e| e.hyperlink.as_deref())
    }
}

/// Rare per-cell metadata, heap-allocated so the common cell stays small.
#[derive(Debug, Default, Clone)]
pub struct CellExtra {
    /// Combining / zero-width codepoints stacked onto the base char.
    /// Soft cap at 128 bytes, hard cap at 256 (warn at the soft cap).
    pub zerowidth: Vec<char>,
    /// OSC 8 hyperlink target, if set.
    pub hyperlink: Option<String>,
    /// OSC 133 prompt marker payload, if set.
    pub prompt: Option<PromptMarker>,
}

const MAX_ZEROWIDTH_BYTES: usize = 256;

impl CellExtra {
    pub fn push_zerowidth(&mut self, c: char) {
        let used: usize = self.zerowidth.iter().map(|c| c.len_utf8()).sum();
        if used + c.len_utf8() <= MAX_ZEROWIDTH_BYTES {
            self.zerowidth.push(c);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMarker {
    Start, // OSC 133 ; A
    End,   // OSC 133 ; B
    Cont,  // OSC 133 ; P
}

/// A row of fixed-width cells.
#[derive(Debug, Clone)]
pub struct Row {
    pub cells: Vec<Cell>,
}

impl Row {
    pub fn new(cols: usize) -> Self {
        Self { cells: vec![Cell::space(); cols] }
    }

    pub fn resize(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::space());
    }

    /// Clear cells in `range`. Used by EL/ECH variants.
    pub fn clear_range(&mut self, range: std::ops::Range<usize>) {
        for cell in &mut self.cells[range] {
            cell.reset();
        }
    }
}

/// Main grid — visible rows plus scrollback, fixed column count.
pub struct Grid {
    /// Rows: `[scrollback..., visible...]`. Visible region is the last
    /// `visible_rows` entries.
    rows: Vec<Row>,
    visible_rows: usize,
    cols: usize,
    max_scrollback: usize,

    // Cursor state
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
    pub cursor_style: CursorStyle,

    /// Saved cursor (DECSC / DECRC)
    saved_cursor: Option<(usize, usize)>,

    /// Scroll region (top, bottom) — 0-based, inclusive
    scroll_top: usize,
    scroll_bottom: usize,

    /// Current drawing attributes used to fill freshly printed cells.
    pub current_fg: TermColor,
    pub current_bg: TermColor,
    pub current_flags: CellFlags,

    /// Alt screen state
    alt_rows: Option<Vec<Row>>,
    alt_cursor: Option<(usize, usize)>,

    /// Modes
    pub origin_mode: bool,    // DEC private 6
    pub auto_wrap: bool,      // DEC private 7
    pub bracketed_paste: bool,
    pub focus_reporting: bool, // DEC private 1004
    pub sync_output: bool,     // DEC private 2026 (flush boundary)
    pub mouse_mode: MouseMode,
    pub cursor_keys_app: bool,

    /// Scrollback offset in **pixels** (rendered by term_gpu). Logical
    /// grid is fixed-cell; scroll is pixel-precise for smooth motion.
    /// See docs/design/gpu-terminal-scroll.md.
    pub scroll_offset_y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorStyle {
    #[default]
    BlockSteady,
    BlockBlink,
    UnderlineSteady,
    UnderlineBlink,
    BeamSteady,
    BeamBlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode { #[default] None, X10, ButtonEvent, AnyEvent, Sgr }

impl Grid {
    pub fn new(cols: usize, rows: usize, max_scrollback: usize) -> Self {
        let visible = (0..rows).map(|_| Row::new(cols)).collect();
        Self {
            rows: visible,
            visible_rows: rows,
            cols,
            max_scrollback,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            cursor_style: CursorStyle::BlockSteady,
            saved_cursor: None,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            current_fg: TermColor::Default,
            current_bg: TermColor::Default,
            current_flags: CellFlags::empty(),
            alt_rows: None,
            alt_cursor: None,
            origin_mode: false,
            auto_wrap: true,
            bracketed_paste: false,
            focus_reporting: false,
            sync_output: false,
            mouse_mode: MouseMode::None,
            cursor_keys_app: false,
            scroll_offset_y: 0.0,
        }
    }

    pub fn cols(&self) -> usize { self.cols }
    pub fn visible_rows(&self) -> usize { self.visible_rows }
    pub fn scrollback_len(&self) -> usize {
        self.rows.len().saturating_sub(self.visible_rows)
    }

    /// Visible-area starting index into `self.rows`.
    fn visible_start(&self) -> usize {
        self.rows.len().saturating_sub(self.visible_rows)
    }

    fn row_mut(&mut self, row: usize) -> &mut Row {
        let idx = self.visible_start() + row;
        &mut self.rows[idx]
    }

    pub fn row(&self, row: usize) -> &Row {
        let idx = self.visible_start() + row;
        &self.rows[idx]
    }

    /// Print a single grapheme base character at the cursor. Handles
    /// autowrap (DEC 7) and advances the cursor by 1 (wide chars by 2,
    /// not shown here for brevity).
    pub fn print(&mut self, c: char) {
        if self.auto_wrap && self.cursor_col >= self.cols {
            self.cursor_col = 0;
            self.linefeed();
        }
        if self.cursor_col >= self.cols {
            self.cursor_col = self.cols - 1;
        }
        let (fg, bg, flags) = (self.current_fg, self.current_bg, self.current_flags);
        let cell = &mut self.row_mut(self.cursor_row).cells[self.cursor_col];
        *cell = Cell {
            c,
            fg,
            bg,
            flags,
            extra: None,
        };
        self.cursor_col += 1;
    }

    /// Append a combining mark to the last printed cell (does not move
    /// the cursor).
    pub fn push_zerowidth(&mut self, c: char) {
        let col = self.cursor_col.saturating_sub(1).min(self.cols - 1);
        let cell = &mut self.row_mut(self.cursor_row).cells[col];
        cell.push_zerowidth(c);
    }

    /// **ECH** — erase N cells at the cursor without moving it.
    pub fn erase_chars(&mut self, n: usize) {
        let start = self.cursor_col;
        let end = (start + n).min(self.cols);
        self.row_mut(self.cursor_row).clear_range(start..end);
    }

    /// **ICH** — insert N blank cells at the cursor, shifting cells right.
    pub fn insert_chars(&mut self, n: usize) {
        let cols = self.cols;
        let row = self.row_mut(self.cursor_row);
        let insert_at = self.cursor_col.min(cols);
        let count = n.min(cols - insert_at);
        row.cells[insert_at..].rotate_right(count);
        for cell in &mut row.cells[insert_at..insert_at + count] {
            cell.reset();
        }
    }

    /// **DCH** — delete N cells at the cursor, shifting cells left.
    pub fn delete_chars(&mut self, n: usize) {
        let cols = self.cols;
        let row = self.row_mut(self.cursor_row);
        let delete_at = self.cursor_col.min(cols);
        let count = n.min(cols - delete_at);
        row.cells[delete_at..].rotate_left(count);
        for cell in &mut row.cells[cols - count..] {
            cell.reset();
        }
    }

    /// **REP** — repeat the last printed character N times.
    pub fn repeat_last(&mut self, n: usize, last: char) {
        for _ in 0..n {
            self.print(last);
        }
    }

    /// Line feed (LF). Wraps into scroll-up at the bottom of the scroll region.
    pub fn linefeed(&mut self) {
        if self.cursor_row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor_row + 1 < self.visible_rows {
            self.cursor_row += 1;
        }
    }

    /// Carriage return (CR).
    pub fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    /// Scroll region up by N rows (rows leave the top into scrollback if the
    /// top equals the scroll region top).
    pub fn scroll_up(&mut self, n: usize) {
        let cols = self.cols;
        for _ in 0..n {
            if self.scroll_top == 0 {
                if self.scrollback_len() >= self.max_scrollback {
                    self.rows.remove(0);
                }
                let insert_idx = self.visible_start() + self.scroll_bottom + 1;
                self.rows.insert(insert_idx, Row::new(cols));
            } else {
                let remove_idx = self.visible_start() + self.scroll_top;
                self.rows.remove(remove_idx);
                let insert_idx = self.visible_start() + self.scroll_bottom;
                self.rows.insert(insert_idx, Row::new(cols));
            }
        }
    }

    /// Scroll region down by N rows (rows leave the bottom and are discarded).
    pub fn scroll_down(&mut self, n: usize) {
        let cols = self.cols;
        for _ in 0..n {
            let remove_idx = self.visible_start() + self.scroll_bottom;
            if remove_idx < self.rows.len() {
                self.rows.remove(remove_idx);
            }
            let insert_idx = self.visible_start() + self.scroll_top;
            self.rows.insert(insert_idx, Row::new(cols));
        }
    }

    /// ED — erase display in one of four modes.
    pub fn erase_display(&mut self, mode: super::parser::EraseMode) {
        use super::parser::EraseMode;
        match mode {
            EraseMode::ToEnd => {
                self.erase_line(EraseMode::ToEnd);
                for r in (self.cursor_row + 1)..self.visible_rows {
                    for cell in &mut self.row_mut(r).cells {
                        cell.reset();
                    }
                }
            }
            EraseMode::ToStart => {
                for r in 0..self.cursor_row {
                    for cell in &mut self.row_mut(r).cells {
                        cell.reset();
                    }
                }
                self.erase_line(EraseMode::ToStart);
            }
            EraseMode::All => {
                for r in 0..self.visible_rows {
                    for cell in &mut self.row_mut(r).cells {
                        cell.reset();
                    }
                }
            }
            EraseMode::Scrollback => {
                let start = self.visible_start();
                self.rows.drain(0..start);
            }
        }
    }

    /// EL — erase line in one of three modes.
    pub fn erase_line(&mut self, mode: super::parser::EraseMode) {
        use super::parser::EraseMode;
        let cols = self.cols;
        let col = self.cursor_col;
        let row = self.row_mut(self.cursor_row);
        match mode {
            EraseMode::All => row.clear_range(0..cols),
            EraseMode::ToEnd => row.clear_range(col..cols),
            EraseMode::ToStart => row.clear_range(0..(col + 1).min(cols)),
            EraseMode::Scrollback => {}
        }
    }

    pub fn enter_alt_screen(&mut self) {
        let cols = self.cols;
        let rows = self.visible_rows;
        let alt: Vec<Row> = (0..rows).map(|_| Row::new(cols)).collect();
        self.alt_rows = Some(std::mem::replace(&mut self.rows, alt));
        self.alt_cursor = Some((self.cursor_row, self.cursor_col));
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    pub fn exit_alt_screen(&mut self) {
        if let Some(rows) = self.alt_rows.take() {
            self.rows = rows;
        }
        if let Some((r, c)) = self.alt_cursor.take() {
            self.cursor_row = r;
            self.cursor_col = c;
        }
    }

    /// Resize the visible grid. Cells in newly-added columns/rows are blank.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        for row in &mut self.rows {
            row.resize(cols);
        }
        while self.rows.len() < self.visible_start() + rows {
            self.rows.push(Row::new(cols));
        }
        self.cols = cols;
        self.visible_rows = rows;
        self.scroll_bottom = rows.saturating_sub(1);
        if self.cursor_row >= rows {
            self.cursor_row = rows.saturating_sub(1);
        }
        if self.cursor_col >= cols {
            self.cursor_col = cols.saturating_sub(1);
        }
    }

    pub fn reset(&mut self) {
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.cursor_visible = true;
        self.current_fg = TermColor::Default;
        self.current_bg = TermColor::Default;
        self.current_flags = CellFlags::empty();
        self.scroll_top = 0;
        self.scroll_bottom = self.visible_rows.saturating_sub(1);
        self.origin_mode = false;
        self.auto_wrap = true;
        self.bracketed_paste = false;
        self.focus_reporting = false;
        self.sync_output = false;
        self.mouse_mode = MouseMode::None;
        self.cursor_keys_app = false;
        for r in 0..self.visible_rows {
            for cell in &mut self.row_mut(r).cells {
                cell.reset();
            }
        }
    }
}
```

**Notable methods missing from this sketch** (implement in commit 5):
`save_cursor`/`restore_cursor`, `set_scroll_region`, `next_tab` /
`previous_tab` for CHT/CBT, `move_cursor_origin_aware` for CUP under
DECOM, `insert_lines` / `delete_lines` (IL/DL — same logic as scroll
but relative to cursor row, not scroll_top).

### 4.8 VT Emulator (emulator.rs)

```rust
// emulator.rs
use crate::grid::{Cell, CursorStyle, Grid, MouseMode, Row};
use crate::parser::{Action, EraseMode, Parser, SgrAction};
use crate::{TermColor, CellFlags};

/// Cursor state for the renderer
#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
    pub style: CursorStyle,
}

/// Render snapshot — all data the GPU needs
pub struct RenderSnapshot {
    pub rows: Vec<Row>,
    pub cursor: CursorState,
    pub title: String,
}

/// Terminal emulator trait
pub trait TerminalEmulator: Send {
    /// Feed raw bytes from PTY
    fn process(&mut self, bytes: &[u8]);
    /// Resize — visible_rows is determined by the renderer
    fn set_visible_rows(&mut self, rows: usize);
    /// Pixel size for the renderer
    fn set_pixel_size(&mut self, width: u32, height: u32);
    /// Snapshot for the GPU renderer
    fn snapshot(&self) -> RenderSnapshot;
    /// Scrollback offset in pixels (0.0 = live tail). See docs/design/gpu-terminal-scroll.md.
    fn scrollback_px(&self) -> f32;
    fn set_scrollback_px(&mut self, offset_px: f32);
    /// Total scrollable height in pixels (visible viewport + scrollback).
    fn total_scroll_height_px(&self) -> f32;
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
    /// Responses to the PTY (DSR, etc.)
    response_buf: Vec<u8>,
}

impl VtEmulator {
    pub fn new(width_px: u32, height_px: u32, scrollback: usize) -> Self {
        // Initial row count — will be updated by the renderer
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

    /// Take and clear the response buffer (for writing back to the PTY)
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

    fn scrollback_px(&self) -> f32 { self.grid.scroll_offset_y }
    fn set_scrollback_px(&mut self, offset: f32) {
        let max = self.grid.scrollback_height_px();
        self.grid.scroll_offset_y = offset.clamp(0.0, max);
    }

    fn mouse_mode(&self) -> MouseMode { self.grid.mouse_mode }
    fn bracketed_paste(&self) -> bool { self.grid.bracketed_paste }
    fn cursor_keys_app(&self) -> bool { self.grid.cursor_keys_app }
    fn title(&self) -> &str { &self.title }
}
```

---

## 5. term_gpu: GPU renderer

> Detailed specification in a separate file: [gpu-renderer-spec.md](gpu-renderer-spec.md)

### 5.1 File structure

```
crates/term_gpu/src/
├── lib.rs              # Public API
├── renderer.rs         # GpuRenderer — main renderer
├── surface.rs          # Surface management (wgpu::Surface)
├── pipeline.rs         # Render pipelines (text + prim)
├── atlas.rs            # GlyphAtlas + ShelfPacker + LRU Cache
├── text.rs             # cosmic-text integration, text shaping
├── instances.rs        # Instance buffers (GlyphInstance, PrimInstance)
├── color.rs            # TermColor → RGBA conversion for GPU
└── shaders/
    ├── text.wgsl       # Text shader
    └── prim.wgsl       # Primitive shader
```

### 5.2 WGSL shaders

#### text.wgsl
```wgsl
struct Uniforms {
    screen_size: vec2<f32>,
    scroll_offset: vec2<f32>,   // {0.0, scroll_offset_y}; see §5.7
    _pad: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

struct GlyphInput {
    @location(0) pos: vec2<f32>,       // pixel position (pre-scroll)
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
    // Subpixel-correct glyph images come from cosmic-text's SubpixelBin
    // (see §5.6) — no shader-side snap needed. Just subtract scroll offset.
    let px = g.pos + q * g.size - uniforms.scroll_offset;
    let ndc = (px / uniforms.screen_size) * 2.0 - 1.0;

    var out: VsOut;
    out.pos = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = mix(g.uv_min, g.uv_max, q);
    out.color = g.color;
    return out;
}

/// Brightness-scaled contrast enhancement, adapted from Windows Terminal's
/// DirectWrite shader (and used by Warp). Lifts the perceived weight of thin
/// glyphs on dark backgrounds without changing the rasterizer.
fn enhance_contrast(alpha: f32, k: f32) -> f32 {
    // k ≈ 0.5 .. 1.0 (lower = stronger). 0.7 is a good default.
    return alpha + alpha * (1.0 - alpha) * k;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Atlas is RGBA8Unorm (see §5.4). For mono glyphs we use the .a channel;
    // for colour glyphs (emoji) we use rgb directly.
    let sample = textureSample(atlas_tex, atlas_samp, in.uv);
    let is_color = sample.r + sample.g + sample.b > 0.0;
    if is_color {
        return sample;  // emoji: pre-multiplied colour
    }
    let alpha = enhance_contrast(sample.a, 0.7);
    return vec4(in.color.rgb, in.color.a * alpha);
}
```

#### prim.wgsl
```wgsl
struct Uniforms {
    screen_size: vec2<f32>,
    scroll_offset: vec2<f32>,   // {0.0, scroll_offset_y}; see §5.7
    _pad: vec4<f32>,
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
    let px = r.pos + q * r.size - uniforms.scroll_offset;
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

/// Instance data for a single glyph
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GlyphInstance {
    pub pos: [f32; 2],       // pixel position
    pub size: [f32; 2],      // glyph size
    pub uv_min: [f32; 2],    // atlas UV min
    pub uv_max: [f32; 2],    // atlas UV max
    pub color: [f32; 4],     // RGBA
}

/// Instance data for a primitive (bg rect, cursor, selection)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PrimInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
}

// Custom unsafe cast in place of bytemuck
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

> Adapted from `warpdotdev/warp` ([crates/warpui/src/rendering/atlas/allocator.rs](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/atlas/allocator.rs), MIT). Two notable departures from the original draft of this spec:
>
> 1. **Atlas format is `RGBA8Unorm`, not `R8Unorm`.** A single atlas serves both monochrome glyphs (data lives in the alpha channel) and colour glyphs (emoji). Without this, emoji silently break.
> 2. **Eviction is a per-glyph frame counter, not a doubly-linked LRU.** Warp uses `MAX_UNUSED_FRAMES = 10`; a glyph not sampled for 10 consecutive frames is dropped on `end_frame()`. This is far simpler than an intrusive linked list and is sufficient for terminal workloads.

```rust
// atlas.rs — shelf bin-packer + frame-counter eviction.

use cosmic_text::CacheKey;
use std::collections::HashMap;

const ATLAS_SIZE: u32 = 1024;       // matches Warp's allocator
const GLYPH_PAD: u32 = 1;           // 1 px H + V padding per glyph
const MAX_UNUSED_FRAMES: u32 = 10;  // Warp's empirical cutoff

/// Shelf-based bin packer (Shelf-Next-Fit).
pub struct ShelfPacker {
    width: u32,
    height: u32,
    /// Y of the current shelf's top edge.
    row_baseline: u32,
    /// Tallest item on the current shelf.
    row_tallest: u32,
    /// Right edge of items already placed on the current shelf.
    row_extent: u32,
}

impl ShelfPacker {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, row_baseline: 0, row_tallest: 0, row_extent: 0 }
    }

    pub fn pack(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let w = w + GLYPH_PAD * 2;
        let h = h + GLYPH_PAD * 2;
        if w > self.width { return None; }
        if self.row_extent + w > self.width {
            // Advance to the next shelf.
            self.row_baseline += self.row_tallest + GLYPH_PAD;
            self.row_extent = 0;
            self.row_tallest = 0;
        }
        if self.row_baseline + h > self.height { return None; }
        let pos = (self.row_extent + GLYPH_PAD, self.row_baseline + GLYPH_PAD);
        self.row_extent += w;
        self.row_tallest = self.row_tallest.max(h);
        Some(pos)
    }

    pub fn reset(&mut self) {
        self.row_baseline = 0;
        self.row_tallest = 0;
        self.row_extent = 0;
    }
}

/// Cached glyph info.
#[derive(Debug, Clone, Copy)]
pub struct CachedGlyph {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub offset_x: f32,
    pub offset_y: f32,
    pub width: f32,
    pub height: f32,
    /// Frame index when this glyph was last sampled. Updated by `get`.
    pub last_used_frame: u32,
}

/// Glyph cache key. Includes subpixel alignment (3 X-variants, see §5.6) so
/// each subpixel offset gets its own atlas entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphCacheKey {
    pub base: CacheKey,
    pub subpixel_x: u8,   // 0..3, see §5.6
}

/// Main atlas struct.
pub struct GlyphAtlas {
    packer: ShelfPacker,
    entries: HashMap<GlyphCacheKey, CachedGlyph>,
    cpu_data: Vec<u8>,       // RGBA8 (4 bytes per pixel)
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    dirty: bool,
    frame: u32,
}

impl GlyphAtlas {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph_atlas"),
            size: wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // RGBA8Unorm: alpha channel for mono glyphs, RGB for emoji. See note above.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&Default::default());

        Self {
            packer: ShelfPacker::new(ATLAS_SIZE, ATLAS_SIZE),
            entries: HashMap::with_capacity(4096),
            cpu_data: vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize],
            texture,
            texture_view,
            dirty: false,
            frame: 0,
        }
    }

    pub fn get_or_insert(
        &mut self,
        key: GlyphCacheKey,
        rasterize: impl FnOnce() -> Option<RasterizedGlyph>,
    ) -> Option<CachedGlyph> {
        if let Some(g) = self.entries.get_mut(&key) {
            g.last_used_frame = self.frame;
            return Some(*g);
        }
        let raster = rasterize()?;
        let (x, y) = self.packer.pack(raster.width, raster.height)?;

        // Copy glyph bitmap into the RGBA8 CPU buffer.
        for row in 0..raster.height {
            for col in 0..raster.width {
                let src = (row * raster.width + col) as usize * raster.bytes_per_pixel();
                let dst = (((y + row) * ATLAS_SIZE + (x + col)) * 4) as usize;
                match raster.format {
                    GlyphFormat::Alpha => {
                        // Mono: write alpha; RGB defaults to 0 — the shader knows to use .a.
                        self.cpu_data[dst]     = 0;
                        self.cpu_data[dst + 1] = 0;
                        self.cpu_data[dst + 2] = 0;
                        self.cpu_data[dst + 3] = raster.data[src];
                    }
                    GlyphFormat::Rgba => {
                        self.cpu_data[dst..dst + 4].copy_from_slice(&raster.data[src..src + 4]);
                    }
                }
            }
        }
        self.dirty = true;

        let cached = CachedGlyph {
            uv_min: [x as f32 / ATLAS_SIZE as f32, y as f32 / ATLAS_SIZE as f32],
            uv_max: [(x + raster.width) as f32 / ATLAS_SIZE as f32, (y + raster.height) as f32 / ATLAS_SIZE as f32],
            offset_x: raster.left as f32,
            offset_y: raster.top as f32,
            width: raster.width as f32,
            height: raster.height as f32,
            last_used_frame: self.frame,
        };
        self.entries.insert(key, cached);
        Some(cached)
    }

    /// Call at the end of each rendered frame. Increments the frame counter
    /// and evicts entries that have not been sampled for `MAX_UNUSED_FRAMES`.
    /// Evictions free entries but do not compact the atlas — `reset()` does
    /// that when fragmentation crosses a threshold.
    pub fn end_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        let cutoff = self.frame.wrapping_sub(MAX_UNUSED_FRAMES);
        self.entries.retain(|_, g| g.last_used_frame.wrapping_sub(cutoff) <= MAX_UNUSED_FRAMES);
    }

    pub fn upload(&mut self, queue: &wgpu::Queue) {
        if !self.dirty { return; }
        queue.write_texture(
            self.texture.as_image_copy(),
            &self.cpu_data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_SIZE * 4),
                rows_per_image: Some(ATLAS_SIZE),
            },
            wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
        );
        self.dirty = false;
    }

    pub fn view(&self) -> &wgpu::TextureView { &self.texture_view }
}

/// Rasterized glyph data (from cosmic-text / swash / fontdb).
pub struct RasterizedGlyph {
    pub data: Vec<u8>,   // Alpha or RGBA depending on `format`
    pub width: u32,
    pub height: u32,
    pub left: i32,       // Bearing X
    pub top: i32,        // Bearing Y
    pub format: GlyphFormat,
}

impl RasterizedGlyph {
    fn bytes_per_pixel(&self) -> usize {
        match self.format {
            GlyphFormat::Alpha => 1,
            GlyphFormat::Rgba => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphFormat {
    /// Single-channel coverage (most fonts).
    Alpha,
    /// Pre-multiplied colour (emoji, CBDT/COLR fonts).
    Rgba,
}
```

### 5.6 Subpixel positioning

> Originally inspired by Warp's `SubpixelAlignment` ([crates/warpui_core/src/fonts.rs#L135-L159](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/fonts.rs#L135-L159), MIT). While prototyping we discovered cosmic-text already ships subpixel positioning, so we use the built-in mechanism instead of hand-rolling Warp's.

`cosmic_text::CacheKey` includes `x_bin: SubpixelBin` and `y_bin: SubpixelBin`, each a 4-variant enum (`Zero`, `One`, `Two`, `Three`). When the shaper produces a glyph, `glyph.physical(offset, scale)` bins the fractional part of the position into one of 16 combinations, and swash rasterizes the image aligned to that subpixel offset.

For us this means **no hand-rolled subpixel code**. Cache by the full `CacheKey` (which already encodes the bins) and the correct image lands in the atlas:

```rust
// text.rs (sketch) — at shape time the cache key is built for us.

for run in buffer.layout_runs() {
    for glyph in run.glyphs {
        let physical = glyph.physical((pen_x, pen_y), scale);
        // physical.cache_key.x_bin / .y_bin are already set.
        let placed = atlas.get_or_insert(physical.cache_key, || {
            rasterize_glyph(font_system, swash_cache, physical.cache_key)
        })?;
        // ... emit GlyphInstance at (physical.x, physical.y) + placed.offset_*
    }
}
```

Trade-off vs Warp: memory cost is ×16 per glyph variant (4×4 bins) vs Warp's ×3 (X-only, snap Y). We accept the extra memory for crisper Y positioning and zero hand-rolled code. The Y snap pattern from earlier drafts (`px.y = floor(px.y)` in `text.wgsl`) is therefore unnecessary and removed.

### 5.7 Scroll uniform & viewport culling

Both `text.wgsl` and `prim.wgsl` read `uniforms.scroll_offset` (a `vec2<f32>` with `{0.0, scroll_offset_y}`) and subtract it before the NDC transform. This is **the entire scroll mechanism on the GPU** — no buffer rebuild, no atlas change, just one uniform write per frame.

CPU-side, the renderer culls rows that fall outside the viewport:

```rust
// renderer.rs (excerpt)

fn build_glyph_instances(&self, lines: &[Line], scroll: &ScrollState) -> Vec<GlyphInstance> {
    let top = scroll.offset_y;
    let bottom = scroll.offset_y + scroll.visible_px;
    let first = self.row_layout.partition_point(|r| r.y_bottom < top);
    let last = self.row_layout.partition_point(|r| r.y_top < bottom);
    (first..last)
        .flat_map(|i| self.glyphs_for_row(&lines[i], &self.row_layout[i]))
        .collect()
}
```

Full scroll integrator (velocity tracking, momentum timer, `EventLoopProxy<CustomEvent::MomentumTick>`) is specified in [docs/design/gpu-terminal-scroll.md](design/gpu-terminal-scroll.md). The 7 momentum constants are copied verbatim from Warp.

### 5.5 Render pass order

1. **Clear** — `wgpu::LoadOp::Clear` with the terminal background colour.
2. **Background rects** — one `PrimInstance` per run of contiguous cells sharing a `bg != Default` (computed in `term_gpu` from the cell grid).
3. **Glyphs** — one `GlyphInstance` per shaped glyph (subpixel-aware, see §5.6).
4. **Cursor** — one `PrimInstance` for the cursor (style controlled by `CursorStyle`).
5. **Selection overlay** — one `PrimInstance` with a semi-transparent colour.
6. **Present** — `output.present()`.

End of frame: call `atlas.end_frame()` so unused glyphs age toward eviction (§5.4).

---

## 6. term_layout: BSP Panel Manager

### 6.1 Structure

Recursive `Box<Node>` BSP — no arena, no `SlotMap`, zero external
dependencies. Two stable id namespaces (`PanelId` for leaves,
`BranchId` for dividers) so a mouse drag can hold a handle across
operations without worrying about reuse. Ratios clamp to
`[MIN_RATIO, MAX_RATIO] = [0.05, 0.95]` so no panel ever ends up
zero-sized.

```rust
// lib.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BranchId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split { Horizontal, Vertical }

pub const MIN_RATIO: f32 = 0.05;
pub const MAX_RATIO: f32 = 0.95;

/// Returned by `dividers()`. `rect` is the 1-px-thin line for drawing,
/// `bounds` is the parent branch's rectangle (used by drag handlers to
/// translate a cursor position into a new ratio).
#[derive(Debug, Clone, Copy)]
pub struct Divider {
    pub id: BranchId,
    pub split: Split,
    pub rect: Rect,
    pub bounds: Rect,
}

// `Node` is private — callers interact only via `PanelTree`, `PanelId`,
// and `BranchId`.

pub struct PanelTree { /* root, id counters, focus */ }

impl PanelTree {
    pub fn new(w: f32, h: f32) -> Self;
    pub fn focus(&self) -> PanelId;
    pub fn set_focus(&mut self, id: PanelId) -> bool;
    pub fn is_empty(&self) -> bool;
    pub fn panels(&self) -> Vec<(PanelId, Rect)>;
    pub fn dividers(&self) -> Vec<Divider>;

    pub fn split(&mut self, target: PanelId, split: Split, ratio: f32) -> Option<PanelId>;
    pub fn close(&mut self, target: PanelId);
    pub fn resize(&mut self, w: f32, h: f32);
    pub fn drag_divider(&mut self, id: BranchId, new_ratio: f32) -> bool;
    pub fn hit_test(&self, x: f32, y: f32) -> Option<PanelId>;
}
```

### 6.2 Semantics

- **Top-anchored on resize.** `resize(w, h)` walks the tree top-down,
  redividing each `Branch`'s new bounds by its stored ratio. Splits
  keep their proportions; no content scrolls in response to window
  size changes.
- **Sibling promotion on close.** `close(target)` collapses the
  `Branch` containing `target` — the surviving sibling subtree
  inherits the Branch's bounds, reflowed via the same
  `recompute_bounds` helper used by `resize`. Closing the only panel
  empties the tree; the calling code reacts to `is_empty()`.
- **Focus moves to the new split on `split`** and to the
  depth-first first remaining leaf when the focused panel is closed.
  `set_focus(id)` lets click handlers re-focus arbitrarily.
- **Half-open hit-test edges.** `hit_test` treats panel rectangles as
  `[x, x+w) × [y, y+h)` so an exact-divider-pixel hit doesn't claim
  membership in two panels.

### 6.3 Status

Phase 4 complete (May 2026). 28 integration tests in
`crates/term_layout/tests/{basic,split,close,resize,hit_test,drag_divider}.rs`.
Visual demo at `crates/term_gpu/examples/layout_demo.rs` — Cmd+D /
Cmd+Shift+D / Cmd+W keyboard shortcuts, click-to-focus, drag-to-resize
dividers. term_layout is wired as a `dev-dependency` of `term_gpu`;
the library itself stays unaware of layout (matching how `term_core`
relates to `term_gpu`'s `render_term`).

---

## 7. Integration with existing code

### Removed files

| File | Reason |
|------|---------|
| `src/ui/terminal.rs` | Replaced by GPU renderer |
| `src/ui/render.rs` | Replaced by GPU renderer |
| `src/ui/layout.rs` | Replaced by term_layout |
| `src/ui/terminal_guard.rs` | crossterm setup — replaced by winit |
| `src/ui/events.rs` | EventHandler via crossterm → winit |
| `src/ui/theme.rs` | Colors → AnsiPalette in term_core |
| `src/ui/header.rs` | Will be moved into the GPU renderer |
| `src/ui/footer.rs` | Will be moved into the GPU renderer |
| `src/pty/emulator/alacritty_impl.rs` | Replaced by term_core::VtEmulator |

### Modified files

| File | Changes |
|------|-----------|
| `src/pty/emulator/mod.rs` | `pub use term_core::emulator::TerminalEmulator;` — trait from term_core |
| `src/pty/session.rs` | `emulator::create()` → `term_core::create_emulator()` |
| `src/pty/handle.rs` | Adapt to the new trait (set_visible_rows instead of set_size) |
| `src/ui/runtime.rs` | Main rewrite: ratatui loop → winit ApplicationHandler |
| `src/ui/app.rs` | Adapt to the new TerminalEmulator trait |
| `src/ui/input.rs` | Adapt classify_key to winit KeyEvent |
| `src/ui/selection.rs` | Pixel-based selection instead of grid-based |
| `Cargo.toml` | Described in section 3 |

### Unchanged files

Everything in `src/proxy/`, `src/config/`, `src/metrics/`, `src/ipc/`, `src/shim/`, `src/args/`, `src/clipboard/`, `src/error/`, `src/shutdown/`.

---

## 8. Roadmap

### Phase 1 — term_core ✅ done (May 2026)
**Files:** `crates/term_core/src/{lib,parser,color,attrs,grid,emulator}.rs`
**Delivered:** Hand-rolled Paul Williams VT parser (0 deps, ~770 LoC),
alacritty-style fixed-cell grid, `VtEmulator` wiring parser→grid.
22 integration tests; `examples/dump.rs` for visual smoke testing.

### Phase 2 — term_gpu base ✅ done (folded into Phase 3 / 3.5)
**Files:** `crates/term_gpu/src/{lib,renderer,pipeline,instances,shaders/}.rs`
**Delivered:** wgpu 24 + winit 0.30 stack, dual `rect`/`text` pipelines,
instanced quads, shared uniform bind group.

### Phase 3 — term_gpu text ✅ done (May 2026)
**Files:** `crates/term_gpu/src/{atlas,text}.rs`
**Delivered:** Variable-width text via cosmic-text, RGBA8 glyph atlas
with Shelf-Next-Fit packer and frame-counter eviction
(§5.4). Subpixel positioning via cosmic-text's built-in `SubpixelBin`
(§5.6) — discovered during implementation, simpler than Warp's
hand-rolled 3-step. `TextShapeCache` with `font_size + scale_factor +
wrap_width` keying, frame eviction at 60.

### Phase 3.5 — Smooth scroll integration ✅ done (May 2026)
**Files:** `crates/term_gpu/src/scroll.rs`, uniform additions to
`text.wgsl` and `prim.wgsl`.
**Delivered:** All seven Warp momentum constants, `EventLoopProxy<
CustomEvent::MomentumTick>` pump via `futures-timer`. `TouchPhase::Ended`
for trackpad gesture end (fixes scroll-fling collision); silence
timeout for wheel mice.

### Phase 4 — term_layout ✅ done (May 2026)
**Files:** `crates/term_layout/src/lib.rs`, tests in `crates/term_layout/tests/`,
demo in `crates/term_gpu/examples/layout_demo.rs`.
**Delivered:** BSP `PanelTree` with `split` / `close` / `resize` /
`hit_test` / `dividers` / `drag_divider` / `set_focus`. Top-anchored
resize, sibling promotion on close, ratio clamp `[0.05, 0.95]`,
separate `PanelId` / `BranchId` namespaces. 28 integration tests,
visual demo with keyboard shortcuts and mouse drag.

### Mini-integration — term_core × term_gpu ✅ done (May 2026)
**Files:** `crates/term_gpu/examples/render_term.rs`.
**Delivered:** End-to-end pipe — stdin → `VtEmulator` → snapshot →
per-cell shaped glyphs at `(col × cell_width_physical, row ×
cell_height_physical)`. Integer physical cell metrics (Warp parity:
`round(advance('M'))`). Cursor (Block/Underline/Bar), background
rects, INVERSE swap. Top-anchored grid resize. Documented in
[docs/articles/warp-gpu-terminal/](articles/warp-gpu-terminal/).

### Phase 5 — Integration (2 weeks) ⬜ pending
**Files:** `src/ui/runtime.rs`, `src/pty/emulator/mod.rs`,
`src/pty/session.rs`, `src/pty/handle.rs`.
**Deliverable:** AnyClaude runs on the GPU terminal; Claude Code
renders correctly. **Blocker:** UX decisions — panels↔sessions
mapping, tab semantics, header/footer chrome.

### Phase 6 — Polish (1 week) ⬜ pending
**Deliverable:** Selection (drag-to-select cells, clipboard), font
fallback configuration, BOLD/ITALIC/UNDERLINE/STRIKE visual
rendering, direct codepoint→glyph_id lookup (avoid per-cell shape
allocation), scrollback navigation in render_term, drop-shadow
shader for overlays (§3.4).

**Progress: ~7 weeks done of ~11 planned.**
