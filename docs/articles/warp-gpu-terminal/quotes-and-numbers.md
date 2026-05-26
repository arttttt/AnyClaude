# Quotes, numbers, and file:line references

Concrete, citable material. Everything below is verbatim from the
sources cited. Use freely in the article.

---

## The 7 momentum constants

> Source: [`crates/warpui/src/windowing/winit/event_loop/mod.rs#L58-L73`](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/windowing/winit/event_loop/mod.rs#L58-L73), MIT

```rust
const MOMENTUM_DECAY:           f32      = 0.968;
const MOMENTUM_DECAY_INTERVAL:  f32      = 0.008;             // 8 ms
const MOMENTUM_FRAME_INTERVAL:  Duration = Duration::from_millis(8);
const MOMENTUM_THRESHOLD:       f32      = 50.0;              // px/s, min to kick momentum
const MOMENTUM_MIN_VELOCITY:    f32      = 1.0;               // px/s, stop threshold
const MOMENTUM_MAX_VELOCITY:    f32      = 2000.0;            // px/s clamp
const MIN_VELOCITY_TIME_DELTA:  f32      = 0.004;             // 4 ms floor
const NUM_PIXELS_PER_LINE:      f32      = 40.0;              // wheel-mouse multiplier
```

Why 40 for `NUM_PIXELS_PER_LINE`? Warp picked the same value as
Chromium / Flutter. `CGEventSourceGetPixelsPerLine` returns ~10,
which feels too slow.

Why 4 ms for `MIN_VELOCITY_TIME_DELTA`? Without it, batched winit
deliveries (5+ wheel events per cycle) produce a near-zero
`time_delta` and `velocity = delta / time_delta` explodes.

## The decay math

```rust
let elapsed = now.duration_since(v.last_update).as_secs_f32();
v.velocity *= MOMENTUM_DECAY.powf(elapsed / MOMENTUM_DECAY_INTERVAL);
```

At 8 ms ticks, after 1 second of momentum: `0.968 ^ 125 ≈ 0.018`. So
near-stop in roughly 1.5 seconds.

## Pixel-based scroll, not line-based

Tmux / our v1 draft:

```rust
pub scrollback_offset: usize,  // lines
```

Warp / our v2:

```rust
pub struct ScrollData {
    pub scroll_start: Pixels,    // f32 wrapper, like CSS scrollTop
    pub visible_px:   Pixels,
    pub total_size:   Pixels,
}
```

> Source: [`crates/warpui_core/src/elements/scrollable.rs#L60-L75`](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/elements/scrollable.rs#L60-L75), MIT

## The whole render-side scroll mechanism

```wgsl
struct Uniforms {
    screen_size:   vec2<f32>,
    scroll_offset: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, in: GlyphInput) -> VsOut {
    let q = QUAD[vi];
    var px = in.pos + q * in.size - uniforms.scroll_offset;
    px.y = floor(px.y);   // snap Y; subpixel X via 3 cached glyph variants
    let ndc = (px / uniforms.screen_size) * 2.0 - 1.0;
    /* ... */
}
```

One uniform write per frame. No vertex/index rebuild. No atlas change.
No layout recompute.

## The atlas

| Parameter | Warp value | Note |
|---|---|---|
| Atlas size | `1024 × 1024` | per atlas; manager allocates more if full |
| Format | `RGBA8Unorm` | mono in alpha, colour for emoji |
| Padding | `1 px H + V` | per glyph |
| Eviction | `MAX_UNUSED_FRAMES = 10` | per-glyph counter, not LRU |
| Subpixel | 3 horizontal variants, snap Y | × 3 memory, no artifacts |

> Source: [`crates/warpui/src/rendering/atlas/allocator.rs`](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/atlas/allocator.rs), [`crates/warpui/src/rendering/glyph_cache.rs#L24`](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/glyph_cache.rs#L24), [`crates/warpui_core/src/fonts.rs#L135-L159`](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/fonts.rs#L135-L159), MIT

## Subpixel positioning, exact

### Warp's approach

```rust
pub const SUBPIXEL_STEPS: u8 = 3;

pub fn subpixel_alignment(pos_x: f32) -> u8 {
    let scaled = pos_x.fract() * SUBPIXEL_STEPS as f32;
    (scaled.round() as i32 % SUBPIXEL_STEPS as i32)
        .rem_euclid(SUBPIXEL_STEPS as i32) as u8
}
```

Plus `px.y = floor(px.y)` in the vertex shader.

### Our approach (cosmic-text built-in)

`cosmic_text::CacheKey` already encodes the subpixel bins:

```rust
pub struct CacheKey {
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub font_size_bits: u32,
    pub x_bin: SubpixelBin,   // 4 variants
    pub y_bin: SubpixelBin,   // 4 variants
    pub flags: CacheKeyFlags,
}

pub enum SubpixelBin {
    Zero, One, Two, Three,
}
```

So we just key the atlas on the full `CacheKey` — no hand-rolled
alignment math, no Y snap in the shader. Trade-off: 16 variants per
glyph (4×4) vs Warp's 3 (X-only, snap Y).

## `enhance_contrast` for thin glyphs on dark themes

```wgsl
fn enhance_contrast(alpha: f32, k: f32) -> f32 {
    // k ≈ 0.5..1.0; 0.7 is a good default
    return alpha + alpha * (1.0 - alpha) * k;
}
```

> Adapted from Windows Terminal's DirectWrite shader.
> Source: [`crates/warpui/src/rendering/wgpu/shaders/glyph_shader.wgsl#L20-L22`](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/wgpu/shaders/glyph_shader.wgsl#L20-L22), MIT

## What Warp explicitly does NOT use

- No compute shaders (anywhere in the render path)
- No indirect draws
- No bindless textures
- No `NSScrollView` / `UIScrollView` FFI
- No `CADisplayLink` / `CVDisplayLink`
- No `hasPreciseScrollingDeltas` direct read (winit reads it)
- No system-momentum integration (`NSEvent.momentumPhase`); Warp
  computes its own inertia and ignores the system's
- No SDF / MSDF glyphs

## TouchPhase fix in our prototype

Before:

```rust
WindowEvent::MouseWheel { delta, .. } => {
    // ... arm 50 ms silence timeout on every wheel
}
```

After:

```rust
WindowEvent::MouseWheel { delta, phase, .. } => {
    let (precise, dy) = match delta {
        MouseScrollDelta::PixelDelta(p) => (true,  p.y as f32),
        MouseScrollDelta::LineDelta(_, v) => (false, v * NUM_PIXELS_PER_LINE),
    };
    self.on_wheel(-dy, phase, precise);
}

match phase {
    TouchPhase::Ended      => self.on_gesture_end(),     // trackpad lift
    TouchPhase::Cancelled  => self.velocity = None,      // gesture interrupted
    _ if !precise          => /* arm silence timeout */, // wheel mouse fallback
    _ => {}                                              // trackpad continuing
}
```

## Standout user quotes

After running the scroll prototype the first time:

> "Уже работает очень круто и приятно. Есть один момент: флинг жест
> сам по себе работает хорошо, но если скроллить не отрывая пальцы от
> трекпада, то как будто скролл и флинг начинают конфликтовать, есть
> некоторое дёргание контента вперёд и назад, пока инерция не
> кончится. В Warp данной проблемы нет, так что можно взять решение
> оттуда."

After the TouchPhase fix:

> "Сейчас проблему не увидел."

After the first text-rendering build:

> "Текст виден, но всё слишком мелкое, раз в 5 как будто увеличить
> надо. Почему не реализовано это [shape caching, CPU culling, font
> fallback]? Всегда есть Варп в качестве референса."

— the single most useful piece of feedback in the project. Triggered
the rule against deferring features Warp ships, and pulled three
"polish later" items into Phase 3 where they belonged.

After the four finishing commits:

> "Всё отлично, обновляем доки для статьи и двигаемся дальше."

## License summary

| Path | License |
|---|---|
| `crates/warpui/` | MIT |
| `crates/warpui_core/` | MIT |
| `crates/sum_tree/` | MIT |
| `crates/editor/` | MIT |
| Other (e.g. `crates/ai`, `crates/warp_terminal` business logic) | AGPL-3.0 |

The rendering stack — atlas, allocator, shaders, scroll integrator,
glyph cache — is MIT. Reusable with attribution.

Attribution comment we add to ported code:

```rust
// Adapted from warpdotdev/warp (MIT)
// Source: crates/warpui/src/rendering/atlas/allocator.rs
```

## Our dependencies (end of Phase 3)

```toml
wgpu          = "24"
winit         = "0.30"
futures       = "0.3"
futures-timer = "3"
glam          = "0.30"
pollster      = "0.4"
cosmic-text   = "0.14"
```

Seven crates. No `tokio` on the render side. No `bytemuck` (manual
`repr(C)` casts). Forking cosmic-text not needed — upstream is enough.

## DPI scaling pattern

Author every instance position and size in **logical pixels**.
Convert once in the vertex shader:

```wgsl
struct Uniforms {
    screen_size: vec2<f32>,    // physical
    scroll_offset: vec2<f32>,  // logical
    scale_factor: f32,
    _pad_a: f32,
    _pad_b: f32,
    _pad_c: f32,
};

@vertex
fn vs_main(...) -> VsOut {
    let px_logical = r.pos + q * r.size - uniforms.scroll_offset;
    let px_physical = px_logical * uniforms.scale_factor;
    let ndc = (px_physical / uniforms.screen_size) * 2.0 - 1.0;
    // ...
}
```

For text: shape at `font_size * scale_factor` so swash rasterizes at
physical density; divide returned glyph positions by `scale_factor`
to get logical back. One conversion at the rasterization boundary.

## WGSL alignment gotcha

`vec3<f32>` in WGSL has **alignment 16, not 12**. Adding a
`_pad: vec3<f32>` at the end of a uniform struct rounds the struct
size up to 48 bytes — even though three floats are only 12 bytes
themselves.

If writing uniforms by hand (no `bytemuck`, no `encase`), use scalar
`f32` pads or `vec4`. Validation error you'll see:

> Buffer is bound with size 32 where the shader expects 48 in
> group[0] compact index 0

## Shape cache + atlas eviction

Identical pattern in both:

```rust
struct Cached {
    payload: T,
    last_used_frame: u32,
}

pub fn end_frame(&mut self) {
    self.frame = self.frame.wrapping_add(1);
    let now = self.frame;
    self.entries.retain(|_, c| {
        now.wrapping_sub(c.last_used_frame) <= MAX_UNUSED_FRAMES
    });
}
```

Atlas: `MAX_UNUSED_FRAMES = 10` (~0.16 s @ 60 fps). Shape cache:
`MAX_UNUSED_FRAMES = 60` (~1 s). Glyphs come and go faster than
shaped lines.

## Frame counts

Phase 3 demo: 1000 stripes + ~100 ruler ticks + 100 row labels
shaped → with culling, ~10 labels actually shape per frame on a 720
logical px viewport. 90% cull rate. cosmic-text shape cost (per the
first frame, before cache is warm): ~5 µs per label × 100 labels =
~0.5 ms total. After cache is warm: ~0 ms.

## Branch state at end of Phase 3

26 commits on `feat/gpu-terminal`. See `timeline.md` §8 for the
breakdown by category.

## term_core (Phase 1) numbers

- 8 atomic feature/docs commits + 1 example commit.
- ~770 LoC for the Paul Williams parser, std-only, 0 external deps.
- ~600 LoC for the Grid (cursor, scroll region, alt screen, all
  edit primitives).
- 30+ `Action` enum variants covering all P0+P1 sequences from the
  research priority list.
- 39 integration tests (20 parser_smoke + 19 emulator_smoke), all
  green.
- 34 commits on `feat/gpu-terminal` at end of Phase 1.

## Cell layout (alacritty-style, Warp-style)

```rust
pub struct Cell {
    pub c: char,                     // 4 bytes
    pub fg: TermColor,               // 4 bytes (Default | Indexed(u8) | Rgb(u8,u8,u8))
    pub bg: TermColor,               // 4 bytes
    pub flags: CellFlags,            // 2 bytes (u16 bitset)
    pub extra: Option<Box<CellExtra>>, // 8 bytes (rare metadata heap-indirected)
}
```

Hot path stays small; combining marks, OSC 8 hyperlinks, OSC 133
prompt markers live on the heap via `extra`.

## The "DA must reply" trap

`CSI c` (Device Attributes) is sent by many apps at startup and
they **block waiting for a reply**. Warp answers with `?6c` (VT102
primary). If you forget to answer at all — silence, the app hangs.

```rust
Action::DeviceAttributes => {
    self.response_buf.extend_from_slice(b"\x1b[?6c");
}
```

Sample app symptoms: cursor frozen, no output, no input.
Easy bug to make, easy to miss because most apps don't send DA.

## OSC 8 (sticky) vs OSC 133 (one-shot) attachment

OSC 8 hyperlinks apply to every cell printed until closed:

```
OSC 8;params;url ST  <text>  OSC 8;;ST
       │                        │
       └─ sticks ─────────────┘
```

OSC 133 prompt markers tag the next cell only:

```
OSC 133;A ST  <one cell tagged>  <subsequent cells un-tagged>
```

Implementation:
- `Grid.current_hyperlink: Option<(String, String)>` — sticky, set/cleared by OSC 8.
- `Grid.next_prompt: Option<PromptMarker>` — `Grid.print` takes (clears) on first attach.
- Either active → `Grid.print` lazily allocates `Cell.extra`.

## Project policy nuance: tests in `tests/`, never `src/`

```
crates/term_core/
├── src/
│   ├── parser.rs       ← no `mod tests` here
│   └── …
└── tests/              ← integration tests live here
    ├── parser_smoke.rs (20 tests)
    └── emulator_smoke.rs (19 tests)
```

Two reasons:
1. `dead_code = "deny"` workspace lint can fire on test-only helpers.
2. Integration tests exercise the public API; unit tests inside
   `src/` can rely on private state and silently break.

Caught the violation in commit 4 (parser) before it landed.

## Locked-in `term_core` decisions

- **Hand-roll Paul Williams state machine, no `vte` dep.** ~770 LoC vs one
  dependency. Worth it for the self-contained crate.
- **Fixed-cell logical grid + variable-width render.** ink demands
  monospace for CUP correctness; cosmic-text shapes variable-width
  in `term_gpu`. Logical and visual models can disagree.
- **Frame-counter eviction reused.** Both `GlyphAtlas` (10 frames)
  and `TextShapeCache` (60 frames). Simpler than LRU.
- **`?6c` (VT102) for DA reply.** Matches Warp; ink doesn't care.
- **DCS / SOS / PM / APC eaten without dispatch.** Out of scope but
  must traverse them so they don't corrupt input.

## Mini-integration (term_core × term_gpu, May 2026)

End-to-end pipe: stdin → `VtEmulator` → `RenderSnapshot` → per-cell
shaped glyphs → wgpu surface.

- **6 commits**: 1 bootstrap + 1 shape + 1 bg/cursor + 2 `term_core`
  fixes + 1 resize wiring.
- **Per-cell shaping**, not per-run. Per-run rode the shaper's
  natural advances and blurred whenever `cell_width × scale_factor`
  was fractional.
- **`cell_width_physical = round(advance_of_'M').max(1.0)`**, integer
  physical pixels. From Warp's `grid_size_util.rs`.
- **`cell_height_physical = round(font_size × 1.3 × scale_factor)`**.
- **Glyph X = `col × cell_width_physical`**. Shaper advances are
  discarded. Mirrors Warp's `paint_line` even in the ligature path.
- **Baseline Y snapped**: `round(origin_y_physical + line.line_y)`
  before `glyph.physical()` so each row hits `SubpixelBin::Y = Zero`.
- **DPI bug**: removed `self.scale_factor = renderer.scale_factor()`
  in commit 1 as YAGNI; commit 2 added the consumer; field stayed
  at `1.0` until restored — shape calls ran at logical-pixel size,
  GPU sampler stretched ×2 on Retina, text read as blurry.
- **Cursor styles**: Block (full cell rect), Underline (bottom 2
  px), Beam (left 2 px). Semi-transparent white (alpha 0.55) so the
  glyph under a block cursor remains legible.
- **INVERSE**: swap `(fg, bg)` per cell. `BOLD`/`ITALIC`/
  `UNDERLINE`/`STRIKE` not yet visually rendered.

## `Grid::resize` bug + UX call (May 2026)

```rust
// Buggy: visible_start() derives from rows.len() & old visible_rows
while self.rows.len() < self.visible_start() + rows {
    self.rows.push(Row::new(cols));
}
// Each push grows rows.len() by 1; visible_start() grows by 1 too;
// condition stays true → infinite loop when rows > visible_rows.
```

Fix: snapshot the bound before the loop.

```rust
let scrollback = self.scrollback_len();
let target = scrollback + rows;
while self.rows.len() < target { self.rows.push(Row::new(cols)); }
```

User-driven semantic call:

> "у меня варп настроен так, что контент внутри него ресайзится,
> но не двигается вверх, вниз или куда либо еще"

So `Grid::resize` ships top-anchored (truncate-bottom on shrink,
pad-bottom on grow, cursor clamp), not alacritty-style
(scroll-into-scrollback). Two-line difference in `resize`,
documented in [[gpu-terminal-architecture]].

## Phase 4 — term_layout numbers

- **6 commits**: bootstrap, split+resize, close, hit_test,
  dividers+drag, set_focus+demo.
- **~250 LoC** in `crates/term_layout/src/lib.rs` (no further
  modules — single-file crate).
- **28 integration tests** across `basic` (3) + `split` (5) +
  `close` (5) + `resize` (4) + `hit_test` (6) + `drag_divider` (7)
  test files.
- **0 external dependencies.** Recursive `Box<Node>`, plain `f32`
  rectangles.
- **2 id namespaces**: `PanelId` (leaves) and `BranchId`
  (dividers). Separate counters in `PanelTree`.
- **Ratio clamp**: `MIN_RATIO = 0.05`, `MAX_RATIO = 0.95`.
- **Demo**: `crates/term_gpu/examples/layout_demo.rs`. Cmd+D /
  Cmd+Shift+D / Cmd+W shortcuts. Click-to-focus. Mouse drag with
  6 px hit tolerance for divider grab. Focus border 2 px,
  semi-transparent white (alpha 0.45).

## Branch state at end of Phase 4

47 commits on `feat/gpu-terminal`. Three crates:

| Crate | LoC (src) | Tests | Deps |
|---|---|---|---|
| `term_core` | ~2000 | 22 | 0 |
| `term_gpu` | ~1300 | — (visual demos) | wgpu, winit, cosmic-text, futures, futures-timer, glam, pollster |
| `term_layout` | ~250 | 28 | 0 |

Three visual demos all running at 120 fps on Retina:

- `scroll_demo` — pixel-scroll with Warp momentum.
- `render_term` — `cat session.log | render_term` shows a real
  terminal grid rendered through cosmic-text.
- `layout_demo` — split / close / drag panels with Cmd-key
  shortcuts.

Phase 5 (integration into `anyclaude`) is pending and blocked on a
UX call (panels ↔ Claude Code sessions, tab semantics, header
chrome).

## term_grid demo (multi-panel PTY, May 2026)

End-to-end terminal: every leaf in `PanelTree` owns a real
`portable-pty` shell.

- **5 commits**: bootstrap, keyboard, multi-panel, per-panel
  resize, docs (reflow added to Phase 6 roadmap).
- **~700 LoC** in `crates/term_gpu/examples/term_grid.rs`.
- **Dev-dep added**: `portable-pty = "0.9"` (same pattern as the
  other examples).
- **Reader thread** per panel: blocking `read()` loop, ships chunks
  via `mpsc::channel`, signals winit with
  `EventLoopProxy::send_event(CustomEvent::BytesArrived(id))`. On
  EOF sends `PanelExited(id)` and dies.
- **`encode_key`** covers: printable text, `Ctrl + letter →`
  0x01..0x1A, `Alt + key →` ESC-prefix, named keys (Enter `\r`,
  Tab, Backspace `\x7f`, Escape, Space, Arrow{Up,Down,Left,Right},
  Home, End, Delete, PageUp, PageDown).
- **Cmd shortcuts**: Cmd+D (vsplit), Cmd+Shift+D (hsplit), Cmd+W
  (close, exits demo if last panel), Cmd+Q (exit). All other Cmd
  combos are swallowed (not forwarded to PTY).
- **Drag sync deferred to `on_mouse_release`**: avoids SIGWINCH
  spam and zsh re-prompt accumulation during continuous drag.
- **Render-side culling**: `populate_panel` and `build_cursor_rect`
  skip glyphs/cursors whose origin lies outside the panel's logical
  bounds — necessary because the PanelTree's rect updates on drag
  motion but the emulator's `(cols, rows)` only updates on release.
- **Known limitation (resolved in reflow phase below)**: at this
  point `Grid::resize` was destructive on column shrink. See the
  "Reflow on column resize" section for the fix.

## Branch state at end of term_grid

53 commits on `feat/gpu-terminal`. Three crates + four end-to-end
demos. All clippy clean, all term_core / term_layout tests passing.

| Demo | What it proves |
|---|---|
| `scroll_demo` | Pixel-scroll + momentum (Phase 3.5) |
| `render_term` | term_core × term_gpu single-panel pipe |
| `layout_demo` | term_layout BSP shape + drag |
| `term_grid` | All three crates + per-panel PTY shells |

## Reflow on column resize (Phase 6 partial, May 2026)

Three atomic commits (`4e5c5e2`, `901ed78`, `e2a4c4b`),
~250 LoC including tests. Fixes the destructive column-shrink in
`Grid::resize` that left "history fragments" after `term_grid`
drag-resize.

**Source reference:** warpdotdev/warp,
`crates/warp_terminal/src/model/grid/flat_storage/index.rs::Index::rebuild`.
The flag encoding (`Flags::WRAPLINE = 1 << 4` in their
`cell.rs`) was the key takeaway — alacritty has the same
encoding; both projects mark soft-wrap on the row's trailing cell,
not as a per-row field.

| Component | Lines |
|---|---|
| `CellFlags::WRAPLINE` (bit 12) | 8 |
| `Grid::print` auto-wrap branch | 6 |
| `Grid::reflow_columns` + helpers | ~110 |
| `Grid::resize` outer (cursor abs tracking, top-anchored grow) | ~20 |
| `tests/reflow.rs` (12 tests) | ~200 |
| `tests/emulator_smoke.rs` (2 added) | ~25 |

**Top-anchored grow formula:**
```rust
let prev_scrollback = self.rows.len().saturating_sub(self.visible_rows);
let visible_increment = rows.saturating_sub(self.visible_rows);
let scrollback_to_keep = prev_scrollback.saturating_sub(visible_increment);
let target = scrollback_to_keep + rows;
```

When `visible_rows` grows, the new vertical space absorbs existing
scrollback (rows slide back into view). When it shrinks,
scrollback length is preserved. Matches the user's "контент не
двигается" expectation.

## Branch state at end of reflow

57 commits on `feat/gpu-terminal`. 56 integration tests in
`term_core` alone (24 emulator + 20 parser + 12 reflow). 28 tests
in `term_layout`. Workspace builds clean, all examples compile,
zero clippy warnings outside of pre-existing.

| Crate | Tests | Notable |
|---|---|---|
| `term_core` | 56 | reflow done |
| `term_gpu` | 0 (visual demos only) | 4 examples |
| `term_layout` | 28 | BSP shape + drag |

## SGR visual flags (Phase 6 partial, May 2026)

Four atomic commits (`79da3d7`, `3b704e9`, `835d680`, `675c92d`),
~200 LoC plus docs. Emulator already emitted `CellFlags` bits;
renderer caught up.

| Component | Lines |
|---|---|
| `TextShapeCache::shape` weight+style param | ~25 |
| `term_gpu::lib.rs` re-exports of `Weight`/`Style` | 4 |
| `populate_panel` SGR plumbing in `term_grid` | ~60 |
| Same in `render_term` | ~60 |
| Callsite updates (3) | ~20 |

**Decoration line positions** (fractions of cell height, 1.3 line-height
ratio, SF Pro metrics):

| Decoration | y fraction | thickness (logical px) |
|---|---|---|
| Underline | 0.78 | 1.0 |
| Double underline (upper) | 0.72 | 0.8 |
| Double underline (lower) | 0.84 | 0.8 |
| Strike | 0.42 | 1.0 |

**Weight / Style mapping**:

| `cell.flags` bit | cosmic-text |
|---|---|
| `BOLD` | `Weight::BOLD` |
| `ITALIC` | `Style::Italic` |

**FAINT** = `color[3] *= 0.5` applied AFTER palette resolution and
BEFORE decoration rects.

**HIDDEN** = skip glyph push, keep bg + decoration rects.

## Branch state at end of SGR

61 commits on `feat/gpu-terminal`. All four demos render full SGR
spec: BOLD, ITALIC, UNDERLINE, DOUBLE_UNDERLINE, STRIKE, FAINT,
HIDDEN. Tests counts unchanged (term_gpu has no unit tests by
project policy; verification is visual).

| Crate | Tests | Notable |
|---|---|---|
| `term_core` | 56 | reflow done |
| `term_gpu` | 0 (visual demos only) | 4 examples, full SGR |
| `term_layout` | 28 | BSP shape + drag |

## Scrollback in `term_grid` (Phase 6 partial, May 2026)

Six functional commits + one revert (`5700301`) + one final fix
(`c5ebc1b`). Net ~250 LoC plus docs. Momentum integrator from
Phase 3.5's `scroll_demo` was the foundation; this work was
multi-panel integration, follow mode, and resolving a convention
bug.

| Commit | Files | Lines |
|---|---|---|
| `0d8b23b` (snapshot) | `term_core/src/{emulator,grid}.rs` + 3 consumer updates | ~35 |
| `ef15d9f` (ScrollState wiring) | `term_grid.rs` | ~235 |
| `2b88388` (render offset) | `term_grid.rs` | ~52 |
| `88426e9` (follow mode) | `term_grid.rs` | ~30 |
| `c23c26e` (jumps) | `term_grid.rs` | ~42 |
| `e56a33e` → `5700301` | revert pair | net 0 |
| `c5ebc1b` (convention fix) | `term_grid.rs` | ~10 |

**Scroll convention** (inverted from `ScrollState` docs):

| `offset_y` | Meaning |
|---|---|
| 0.0 | BOTTOM (cursor visible, follow mode target) |
| `max_offset` | TOP of scrollback (oldest content) |

**Follow-mode predicate**:

```rust
was_at_bottom = panel.scroll.offset_y <= SCROLL_BOTTOM_EPSILON
SCROLL_BOTTOM_EPSILON = 0.5  // logical pixels
```

**App-level in-flight gesture**: single `scrolling_panel:
Option<PanelId>` + single `momentum_abort: Option<AbortHandle>`.
`CustomEvent::MomentumTick(PanelId)` carries the panel id so
stale ticks after focus change are dropped.

## Branch state at end of scrollback

69 commits on `feat/gpu-terminal`. `term_grid` is now usable as a
real terminal: long shell output scrolls cleanly with trackpad
momentum, follow mode keeps the cursor visible while the shell
prints, `Cmd+Home`/`Cmd+End` jump between scrollback top and
bottom. Phase 6 remaining: selection, clipboard, font fallback,
performance pass.

| Crate | Tests | Notable |
|---|---|---|
| `term_core` | 56 | reflow done, snapshot exposes full buffer |
| `term_gpu` | 0 (visual demos only) | 4 examples, full SGR, scrollback |
| `term_layout` | 28 | BSP shape + drag |

## Selection in `term_grid` (Phase 6 partial, May 2026)

Three commits, ~400 LoC plus docs.

| Commit | Files | Lines |
|---|---|---|
| `773d37b` (selection + drag + render) | `term_grid.rs` | ~248 |
| `6598d7f` (Esc clear) | `term_grid.rs` | ~13 |
| `d82418f` (double/triple click) | `term_grid.rs` | ~153 |

**Color** (Warp's `text_selection_color`):

| Channel | Value |
|---|---|
| R | 118/255 ≈ 0.463 |
| G | 167/255 ≈ 0.655 |
| B | 250/255 ≈ 0.980 |
| A | 0.4 |

**Multi-click threshold**: 400 ms at the same cell + same panel.
Counter wraps 1 → 2 → 3 → 1.

**Word boundary characters** (lifted from
`crates/warpui_core/src/text/words.rs::DEFAULT_WORD_BOUNDARY_CHARS`):

```
` ~ ! @ # $ % ^ & * ( ) - = + [ { ] }
\ | ; : ' " , . < > / ? « »
```

Whitespace also counts as a boundary.

**Selection clear triggers**:

| Trigger | Cleared? |
|---|---|
| PTY bytes arrive | Yes (unless mid-drag in that panel) |
| `Grid::resize` (cols or rows change) | Yes |
| Esc keypress | Yes (Esc also forwarded to PTY) |
| Mouse-release on no-drag click | Yes |
| User scroll (wheel, momentum, Cmd+Home/End) | No |
| Click off the selection (focus another panel) | Starts a new empty selection in target panel |

## Branch state at end of selection

73 commits on `feat/gpu-terminal`. Selection works for drag,
double-click word, triple-click row. Copy is still missing
(clipboard is the next deliverable). Phase 6 remaining:
clipboard, font fallback, performance pass.

| Crate | Tests | Notable |
|---|---|---|
| `term_core` | 56 | reflow done, snapshot exposes full buffer |
| `term_gpu` | 0 (visual demos only) | 4 examples, full SGR, scrollback, selection |
| `term_layout` | 28 | BSP shape + drag |

## Clipboard (Phase 6 partial, May 2026)

Seven commits, ~700 LoC + tests, new sibling crate. Full
parity with Warp's `warpui_core::clipboard` (data model +
heuristics) and `warpui::platform::mac::clipboard` (NSPasteboard
FFI). Image paste lands as temp-file paths so Claude Code's
image input works.

| Commit | What |
|---|---|
| `abf16f9` | `term_clipboard` crate skeleton + 11 in-memory tests |
| `f11561d` | MacClipboard plain text + 1 ignored mac smoke |
| `6e36d85` | MacClipboard HTML + images + file paths |
| `a68e174` | term_grid Cmd+C → selection_to_text → clipboard |
| `e4563fe` | term_grid Cmd+V plain text + ALL shortcuts via physical_key |
| `048c55d` | term_clipboard image utilities + MIME priority list at full warp parity |
| `cacf9f3` | term_grid Cmd+V full paste flow: text + paths + image-data-to-temp |

**Image MIME priority** (matches Warp's `CLIPBOARD_IMAGE_MIME_TYPES`):

| Order | MIME |
|---|---|
| 1 | image/png |
| 2 | image/jpeg |
| 3 | image/jpg |
| 4 | image/gif |
| 5 | image/webp |

**Image extension filter** (matches Warp's `IMAGE_EXTENSIONS`):

```
.png .jpg .jpeg .gif .webp
```

**Temp-file path**: `$TMPDIR/term_grid_clipboard_<nanos>.<ext>`
where `<ext>` is derived from the picked MIME and `<nanos>` is
`SystemTime::UNIX_EPOCH`-relative nanoseconds.

**Selection color** (Warp's `text_selection_color`):
`rgba(118, 167, 250, 0.4)`.

**Shell escaping**: single-quote with internal `'` → `'\''`,
POSIX-compatible across bash/zsh/sh/dash.

## Branch state at end of clipboard

78 commits on `feat/gpu-terminal`, 4 crates.

| Crate | Tests | Notable |
|---|---|---|
| `term_core` | 56 | reflow, snapshot exposes full buffer |
| `term_gpu` | 0 (visual demos only) | 4 examples, full SGR, scrollback, selection, clipboard |
| `term_layout` | 28 | BSP shape + drag |
| `term_clipboard` | 15 + 1 ignored mac | trait + InMemoryClipboard + MacClipboard, full warp parity |

`term_grid` is now feature-complete enough to use as a real
terminal: type / scroll / momentum / drag-select / double-click
word / triple-click line / Cmd+C / Cmd+V (plain text + image
filepaths + image-data-to-temp). Phase 6 remaining: font
fallback, performance pass (codepoint → glyph_id direct lookup).

## License attribution snippet

For files containing code ported from Warp:

```rust
// Adapted from warpdotdev/warp (MIT)
// Source: crates/warpui/src/rendering/atlas/allocator.rs
```
