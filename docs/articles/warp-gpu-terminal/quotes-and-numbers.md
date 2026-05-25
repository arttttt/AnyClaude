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

## License attribution snippet

For files containing code ported from Warp:

```rust
// Adapted from warpdotdev/warp (MIT)
// Source: crates/warpui/src/rendering/atlas/allocator.rs
```
