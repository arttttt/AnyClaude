# Warp VT/ANSI Parser Research

> Research conducted May 2026 against `warpdotdev/warp` to inform the `term_core` crate (Phase 1 of the GPU terminal roadmap). Companion to [`warp-rendering-research.md`](warp-rendering-research.md).

## 1. What Warp uses

**Parser dependency**: Warp uses the [`vte`](https://github.com/alacritty/vte) crate (originally an Alacritty subproject), pinned to a **Warp fork** at `https://github.com/warpdotdev/vte.git` rev `4b399c87b6...`. From [`crates/warp_terminal/Cargo.toml`](https://github.com/warpdotdev/warp/blob/main/crates/warp_terminal/Cargo.toml) line 28: `vte.workspace = true`. The workspace declares it as `default-features = false` git dep.

**Architecture**: Warp wraps `vte::Parser` (a Paul Williams state-machine implementation) and implements `vte::Perform` themselves. Two-file split:

- [`app/src/terminal/model/ansi/mod.rs`](https://github.com/warpdotdev/warp/blob/main/app/src/terminal/model/ansi/mod.rs) (1869 lines) — `Processor` wraps `VteParser`, the `Performer` impl `vte::Perform` dispatches every CSI/OSC/ESC/DCS/APC into a `Handler` trait. Entry point is `Processor::parse_bytes()` at line 426.
- [`crates/warp_terminal/src/model/ansi/control_sequence_parameters.rs`](https://github.com/warpdotdev/warp/blob/main/crates/warp_terminal/src/model/ansi/control_sequence_parameters.rs) — `Mode::from_primitive` (DEC private mode lookup, line ~110) and `attrs_from_sgr_parameters` (SGR table at line ~480). File-level comment says "adapted from the vte crate (an Alacritty project) under the Apache license".
- [`crates/warp_terminal/src/model/escape_sequences.rs`](https://github.com/warpdotdev/warp/blob/main/crates/warp_terminal/src/model/escape_sequences.rs) — **output-side only** (keystroke → pty bytes); not a parser.

**Adapted from Alacritty**: `mode.rs`, `grid/cell.rs`, `grid/row.rs` are all marked "adapted from the alacritty_terminal crate under the Apache license" — Warp copied & extended Alacritty's model rather than depending on `alacritty_terminal`.

**Grid model**: **Fixed-cell grid, alacritty-style.** `Row { inner: Vec<Cell>, occ: usize }` ([`crates/warp_terminal/src/model/grid/row.rs#L15`](https://github.com/warpdotdev/warp/blob/main/crates/warp_terminal/src/model/grid/row.rs#L15)). Each `Cell` is exactly 24 bytes (`char c`, `Color fg`, `Color bg`, `Flags flags`, `Option<Box<CellExtra>>`) — heavily optimized for memory. The `Cell::extra` `Box` holds zero-width grapheme accumulation and prompt markers — a clever design where rare per-cell metadata is heap-indirected. **No variable-width spans.** Zero-width / combining characters are accumulated onto the base cell via `Cell::push_zerowidth` (cap 256 bytes, warn at 128). Wide characters use `WIDE_CHAR` + `WIDE_CHAR_SPACER` flags; the spacer cell is a placeholder. The `occ` field acts as the per-row dirty high-water mark — implicit damage tracking.

## 2. What our (original) spec is missing that Warp does

Ignoring Warp's bespoke OSCs (9277-9280, 781378) and tmux control mode:

### CSI not in original spec

- `@` ICH — insert blank chars (line 1339)
- `b` REP — repeat last char N times (1344). ink/yoga uses this for box drawing.
- `c` DA — primary device attributes (1354) — apps **send this and wait**; without a response, some apps hang.
- `d` VPA — vertical position absolute (1358)
- `E`/`F` CNL/CPL — cursor next/prev line (1359-1360)
- `f` HVP — same as CUP (1374)
- `g` TBC — tab clear (1362)
- `I`/`Z` CHT/CBT — cursor forward/backward tabs (1392, 1555)
- `n` DSR — already in spec but Warp also responds; ink polls `CSI 6 n` for cursor pos
- `P` DCH — delete chars (1454) — frequently used by terminal libs for in-place edits
- `p` DECRQM — request mode (1455) — apps query support for features like sync output
- `q` (space prefix) DECSCUSR — set cursor style block/underline/beam (1481)
- `r` DECSTBM — already in spec
- `s`/`u` SCOSC/SCORC — save/restore cursor (1514, 1525)
- `t` window manipulation 14/18/22/23 — pixel/char text-area size, push/pop title (1518)
- `X` ECH — erase chars (1554). Critical: ink uses this to clear partial lines without affecting cursor.

### SGR not in original spec

- `4;2` DOUBLE_UNDERLINE; `4;0` cancel underline
- `5`/`6` BlinkSlow/BlinkFast
- `8` Hidden; `28` CancelHidden
- `21` CancelBold (different from `22`)

### DEC private modes not in original spec

- `3` DECCOLM — 80/132 cols (rare but apps send it)
- `6` DECOM — origin mode (affects CUP semantics within scrolling region!)
- `7` DECAWM — autowrap
- `12` blinking cursor
- `1004` focus in/out reporting — apps subscribe via `[?1004h`, expect `CSI I` / `CSI O` on focus events. Claude Code probably uses this.
- `1005` UTF-8 mouse (legacy)
- `1007` alternate scroll — wheel produces arrow keys in alt screen
- `1042` urgency hints
- `2026` SYNC OUTPUT (DECSET 2026) — major modern feature, batches output frames

### OSC not in original spec

- `OSC 4` palette index set
- `OSC 7` CWD (`file://host/path`) — used by shell integrations
- `OSC 8` hyperlinks — **the big one**. Warp falls through to `unhandled`. So Warp does NOT actually handle OSC 8.
- `OSC 9` (iTerm/ConEmu desktop notifications + numeric subcodes for ConEmu)
- `OSC 10/11/12` foreground/background/cursor color get-set (with `?` query)
- `OSC 50` cursor shape (legacy alternative to DECSCUSR)
- `OSC 52` clipboard
- `OSC 104/110/111/112` color resets
- `OSC 133` FinalTerm prompt markers (shell integration)
- `OSC 777` urxvt/foot notifications
- `OSC 1337` iTerm inline images

### ESC simple sequences not in original spec

- `ESC D` IND — line feed
- `ESC E` NEL — newline (LF + CR)
- `ESC H` HTS — set horizontal tab stop
- `ESC Z` DECID — terminal ID (obsolete CSI c)
- `ESC =` / `ESC >` keypad app mode set/reset
- `ESC #8` DECALN — alignment test
- `ESC ( B` etc. — charset designation (G0=ASCII, G0=line drawing) — handled in 1571-1588.

### C0 controls not in original spec

- `0x0B VT` and `0x0C FF` — Warp treats as LF (line 808)
- `0x0E SO` / `0x0F SI` — switch G0/G1 charsets (line 813-814)
- `0x1A SUB` — abort current escape; some apps inject this on parser-reset

### Synchronized output

Warp implements `DECSET 2026` with a 150 ms timeout and 2 MiB buffer (lines 65-78). Frames are buffered; output is delivered atomically.

### Kitty keyboard protocol

Warp parses `CSI = / > / < / ?` followed by `u`. Five flags (`KEYBOARD_DISAMBIGUATE_ESCAPE`, etc.) in `mode.rs`. Claude Code most likely uses this for modifier keys (Shift+Enter, Ctrl+Backspace).

## 3. Recommendations for `term_core`, ordered by importance

### P0 — required for Claude Code to look correct

1. **`CSI X` ECH (erase chars)** — ink uses it constantly.
2. **`CSI P` DCH (delete chars)** and **`CSI @` ICH (insert chars)** — needed for any line-editing redraw.
3. **`CSI d` VPA + `CSI E`/`F` CNL/CPL** — ink's positioning primitives.
4. **`CSI c` DA + `CSI n` DSR responses** — write `\x1b[?6c` or `\x1b[?1;2c` back to the PTY on `CSI c`; otherwise apps may hang on startup.
5. **DEC private 7 (DECAWM autowrap), DEC private 6 (DECOM origin)** — CUP semantics break inside scrolling regions if you ignore these.
6. **`CSI b` REP** — small, cheap, used surprisingly often by curses-style apps.

### P1 — modern UX, low complexity

7. **`CSI Ps SP q` DECSCUSR** — cursor shape (block/beam/underline + blink). Claude Code definitely sets this.
8. **DEC private 1004 + emit `CSI I` / `CSI O`** — focus reporting. ink uses it to dim background panels.
9. **`OSC 7` CWD parsing** — shell integration; tab title and worktree integration.
10. **`OSC 133` prompt markers** — shell integration; lets you make blocks like Warp does. Note from Warp's `OSC_133` handling: simple `A`/`B`/`P` payload structure (see `PromptMarker::try_from` in control_sequence_parameters.rs).
11. **`OSC 8` hyperlinks** — even though Warp does NOT support these, modern apps do (lazygit, gh, etc.). Recommend storing the URL on the cell's attrs as a separate optional field. Skip unless target apps emit it.

### P2 — robustness

12. **`ESC D / E / M`** (IND/NEL/RI is already covered by ESC M) — trivial.
13. **`ESC = / >`** keypad mode — small state flag, affects key encoding.
14. **`SGR 4;2` (double underline), `4;0` cancel** — extended SGR forms.
15. **DECSET 2026 sync output** — only relevant if you observe flicker. Spec it as a buffer-and-flush, NOT a renderer change.
16. **`ESC ( B` / `ESC ( 0`** — DEC charset designation. Add the bare minimum: G0 ASCII vs G0 line-drawing table (see Warp's `StandardCharset::map` for the 32-glyph translation in `control_sequence_parameters.rs` ~line 440).

### P3 — explicitly defer

17. Kitty keyboard protocol (`CSI u`) — implement only if Claude Code uses modifiers beyond Alt-prefixed. Start by ignoring and add later.
18. `OSC 52` clipboard, `OSC 10/11/12` color queries — niche.

### Parser state machine notes

Don't try to invent your own state machine. The Paul Williams diagram (https://vt100.net/emu/dec_ansi_parser) is the well-trodden ground that `vte` implements. Hand-rolling it as zero-deps in `std` is straightforward — ~500 LoC.

Key states: GROUND, ESCAPE, ESCAPE_INTERMEDIATE, CSI_ENTRY, CSI_PARAM, CSI_INTERMEDIATE, CSI_IGNORE, OSC_STRING, DCS_* (can ignore-and-eat-until-ST), SOS/PM/APC (ignore-and-eat).

No SIMD tricks in Warp — they iterate one byte at a time. UTF-8 decoding happens inside the `vte::Parser::advance` loop on the printable-character path.

### Damage tracking

Warp's `Row::occ` (high-water mark) is a damage-tracking optimization for renderers that only redraw dirty cells. **Our `term_gpu` does full re-render** — we do not need `occ` and can drop it. But keep the cursor-moved + viewport-scrolled + alt-screen-swapped events as explicit signals so the GPU side knows when to invalidate batched state.

## 4. What we NOT do

1. **Tmux control mode parsing** (Warp's `TmuxControlModeParser`, lines 1652-1827 of mod.rs) — entirely Warp-specific UX. We wrap Claude Code, not tmux. Skip.
2. **DCS hex/JSON shell hooks** (`dcs_hooks.rs`, `WARP_OSC_MARKER` 9277-9280) — Warp's proprietary shell integration protocol. Not applicable.
3. **iTerm inline images** (`OSC 1337`) and **Kitty image APC** (`apc_*` dispatch lines 1617-1649) — Claude Code doesn't emit images. Defer indefinitely.
4. **`CSI = u` etc. with Windows conditional disable** — target is macOS/Linux PTY; no ConPTY concerns.
5. **`OSC 4` palette manipulation + `OSC 10/11/12` dynamic colors with `?` query** — Warp does these for theme integration. Claude Code does not change palette.
6. **`OSC 9277..9280` Warp's own OSC ID space** — Warp-only.
7. **Forking `vte`** — Warp pinned a fork for their needs. We're writing from scratch, so this doesn't apply.
8. **`Cell` struct memory layout obsession** (24-byte packing) — Warp does this because they have multi-million-cell scrollback. For Claude Code (typically <10k cells visible), a 32-byte cell with a `String` for graphemes loses nothing.
9. **Charset G2/G3** — Warp handles `ESC * / ESC +` for completeness; G0+G1 alone covers 99.9% of TUI apps.

## 5. Locked-in decisions for `term_core`

Following this research (user approval May 2026):

- **Hand-roll Paul Williams state machine, 0 external deps.** ~500 LoC. Match `vte`'s state machine but not its API.
- **Fixed-cell logical grid (alacritty-style)** — `Cell { c, fg, bg, flags }`, `Grid<Cell>`. Variable-width rendering happens in `term_gpu`, not in `term_core`.
- **Include all P0 + P1 sequences from §3.** P2 deferred unless observed as needed in real Claude Code traces. P3 explicitly out of scope.
- **Skip everything in §4** (Warp features that don't apply).

## Reference file paths (Warp commit `fc110333`)

- https://github.com/warpdotdev/warp/blob/main/crates/warp_terminal/Cargo.toml
- https://github.com/warpdotdev/warp/blob/main/app/src/terminal/model/ansi/mod.rs (Performer impl)
- https://github.com/warpdotdev/warp/blob/main/crates/warp_terminal/src/model/ansi/control_sequence_parameters.rs (Mode, SGR)
- https://github.com/warpdotdev/warp/blob/main/crates/warp_terminal/src/model/mode.rs (TermMode flags)
- https://github.com/warpdotdev/warp/blob/main/crates/warp_terminal/src/model/grid/cell.rs (Cell layout)
- https://github.com/warpdotdev/warp/blob/main/crates/warp_terminal/src/model/grid/row.rs (Row + occ damage)
