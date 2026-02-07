# Terminal Emulator Crate Comparison

> Research conducted Feb 2026. Evaluating alternatives to `vt100` (v0.16.2) for in-memory terminal emulation in the TUI wrapper.

## Problem

The `vt100` crate (v0.16.2, latest) does not handle modern escape sequences used by Claude Code's status line (OSC 8 hyperlinks, Kitty protocol, synchronized output, etc.). This causes control characters (`^E`, `^O`, `^R`, `^C`) to leak into the rendered terminal body.

## Current Implementation

```rust
// vt100 API used in ClaudeWrapper
vt100::Parser::new(rows, cols, scrollback_len)
parser.process(&buffer[..count])
screen.cell(row, col).contents()    // &str
screen.cell(row, col).fgcolor()     // Color
screen.scrollback()                 // usize
screen_mut().set_scrollback(offset)
screen_mut().set_size(rows, cols)
```

Key requirements:
- **In-memory rendering**: No GUI, parse PTY output into a cell grid
- **Scrollback**: Navigate history (used for scroll up/down)
- **Modern escape sequences**: OSC 8, Kitty keyboard, synchronized output
- **Cross-platform**: macOS, Linux (Windows optional)
- **Performance**: Real-time PTY output processing

## Candidates Evaluated

### 1. `vt100` v0.16.2 (current)

- **Author**: doy
- **License**: MIT
- **Status**: Latest version, no further updates planned
- **Scrollback**: Yes
- **Escape support**: Basic VT100/VT220, OSC 52 (clipboard), CNL/CPL, dim
- **Missing**: OSC 8 (hyperlinks), Kitty protocol, synchronized output, many modern sequences
- **API**: Simple, clean — `Parser` + `Screen` + `Cell`
- **Verdict**: Insufficient for modern CLI tools like Claude Code

### 2. `alacritty_terminal` v0.25.1 (recommended)

- **Author**: Alacritty project (chrisduerr et al.)
- **License**: Apache-2.0
- **Last updated**: Oct 2025
- **Scrollback**: Yes — `Grid` with history, `Scroll::Delta(n)`
- **Escape support**: Full xterm compatibility — OSC 8, OSC 52, keyboard modes, synchronized output, cursor styles, hyperlinks, sixel (partial)
- **Dependencies**: `vte 0.15`, `parking_lot`, `bitflags`, `unicode-width`, `log`, `regex-automata` — moderate
- **Documentation**: 63% on docs.rs
- **Platforms**: macOS, Linux, Windows

**API overview:**
```rust
// Core type
pub struct Term<T: EventListener> { ... }

// Feed PTY output via vte parser
for byte in &bytes { parser.advance(&mut term, *byte); }

// Read grid
let cell = &term.grid()[line][col];
cell.c           // char
cell.fg / cell.bg // Color
cell.flags        // Flags (BOLD, ITALIC, UNDERLINE, etc.)

// Scrollback
term.grid().history_size()
term.scroll_display(Scroll::Delta(n))

// Resize with text reflow
term.resize(SizeInfo::new(cols, rows))

// Efficient re-rendering
let damage = term.damage();  // only changed lines
```

**Pros:**
- Production-tested (Alacritty — millions of users)
- Damage tracking (`TermDamage`) for efficient re-rendering
- Published on crates.io with stable API
- Active maintenance
- `tty` / `event_loop` modules are optional — we only need `Term` + `Grid`

**Cons:**
- Requires `EventListener` trait implementation (clipboard, title, bell events)
- Grid-based API differs from vt100's `cell(row, col)` — requires `TerminalBody` rewrite
- Some unused modules increase binary size slightly

**Migration effort: Medium** — rewrite `terminal.rs`, `handle.rs`, `session.rs`

### 3. `tattoy-wezterm-term` (fork of wezterm_term)

- **Author**: Tattoy project (unofficial fork)
- **License**: MIT
- **Last updated**: Jul 2025
- **Scrollback**: Yes
- **Escape support**: Excellent (WezTerm is one of the most compatible emulators)
- **Dependencies**: Heavy — entire `wezterm-*` + `termwiz` ecosystem
- **Documentation**: Does NOT build on docs.rs

**Pros:**
- WezTerm is also production-tested
- Used by `shadow-terminal` and `tattoy` projects

**Cons:**
- Unofficial fork — not maintained by WezTerm author (wez)
- Original `wezterm_term` is explicitly not published to crates.io (unstable API)
- Docs don't build — poor DX
- Heavy dependency tree
- Unpredictable long-term maintenance

**Migration effort: Medium-High** — similar scope to alacritty_terminal, but with worse docs

### 4. `shadow-terminal` (wrapper around wezterm_term)

- **Author**: Tattoy project
- **Last updated**: Jul 2025
- **Scrollback**: **NO** — listed as roadmap TODO
- **Verdict**: Eliminated due to missing scrollback

### 5. `avt` (asciinema virtual terminal)

- **Author**: asciinema project
- **Last updated**: Oct 2025
- **Scrollback**: No — designed for recording/playback
- **Verdict**: Wrong use case

### 6. `memterm`

- **Author**: orhanbalci
- **Last updated**: Jan 2025
- **Scrollback**: Unknown, likely no
- **Escape support**: Basic
- **Verdict**: Too immature

### 7. `par-term-emu-core-rust`

- **Author**: paulrobello
- **Last updated**: Dec 2025
- **Escape support**: VT100-VT520 compatibility claim
- **Verdict**: Too new, unproven

## Comparison Matrix

| Feature | vt100 | alacritty_terminal | tattoy-wezterm-term |
|---|---|---|---|
| Scrollback | Yes | Yes | Yes |
| OSC 8 (hyperlinks) | No | Yes | Yes |
| Kitty keyboard | No | Yes | Yes |
| Synchronized output | No | Yes | Yes |
| Damage tracking | No | Yes | No |
| Text reflow on resize | No | Yes | Yes |
| crates.io | Yes | Yes | Yes (fork) |
| docs.rs | Yes | Yes | No |
| Maintenance | Stale | Active | Uncertain |
| Dep weight | Light | Moderate | Heavy |
| Production users | Some | Millions | Hundreds |

## Recommendation

**`alacritty_terminal`** is the clear choice:

1. **Maturity**: Powers Alacritty, one of the most popular terminal emulators
2. **Escape support**: Handles everything Claude Code throws at it
3. **Scrollback**: Full support with history
4. **Damage tracking**: Bonus — enables efficient re-rendering
5. **Maintenance**: Actively maintained, published on crates.io
6. **Dependencies**: Reasonable weight

### Alternative: Quick Fix

If migration is not immediately feasible, a quick fix is to filter control characters in `TerminalBody::render()`:

```rust
// In terminal.rs, before set_symbol:
let contents = cell.contents();
if contents.chars().any(|c| c.is_control() && c != '\t') {
    continue; // skip cells with leaked control chars
}
cell_ref.set_symbol(&contents).set_style(style);
```

This masks symptoms but doesn't fix root cause.

## Migration Plan (alacritty_terminal)

### Files to modify:
1. `Cargo.toml` — replace `vt100` with `alacritty_terminal`
2. `src/pty/session.rs` — replace `vt100::Parser` with `alacritty_terminal::Term`
3. `src/pty/handle.rs` — update scrollback API
4. `src/ui/terminal.rs` — rewrite `TerminalBody` for alacritty's `Grid` API
5. Tests

### Key changes:
- Implement `EventListener` trait (can be minimal — just log/ignore events)
- Replace `parser.process(&bytes)` with `vte::Parser` → `Term` feeding
- Replace `screen.cell(row, col)` with `grid[line][col]`
- Replace `Cell::contents()` (String) with `Cell::c` (char)
- Replace `Color::Idx/Rgb` mapping with alacritty's color types
- Update scrollback: `set_scrollback(n)` → `scroll_display(Scroll::Delta(n))`

### Estimated scope:
- ~200 lines of code changes
- 3-5 files affected
- No architectural changes — drop-in replacement at the parser level
