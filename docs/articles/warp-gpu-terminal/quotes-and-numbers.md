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

```rust
pub const SUBPIXEL_STEPS: u8 = 3;

pub fn subpixel_alignment(pos_x: f32) -> u8 {
    let scaled = pos_x.fract() * SUBPIXEL_STEPS as f32;
    (scaled.round() as i32 % SUBPIXEL_STEPS as i32)
        .rem_euclid(SUBPIXEL_STEPS as i32) as u8
}
```

Cache key:

```rust
struct GlyphCacheKey {
    base:        CacheKey,
    subpixel_x:  u8,  // 0..3
}
```

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

After running the prototype the first time:

> "Уже работает очень круто и приятно. Есть один момент: флинг жест
> сам по себе работает хорошо, но если скроллить не отрывая пальцы от
> трекпада, то как будто скролл и флинг начинают конфликтовать, есть
> некоторое дёргание контента вперёд и назад, пока инерция не
> кончится. В Warp данной проблемы нет, так что можно взять решение
> оттуда."

After the TouchPhase fix:

> "Сейчас проблему не увидел."

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

## Our prototype dependencies (final)

```toml
wgpu          = "24"
winit         = "0.30"
futures       = "0.3"
futures-timer = "3"
glam          = "0.30"
pollster      = "0.4"
```

Six crates. No `tokio` on the render side. No `bytemuck` (manual
`repr(C)` casts). No `cosmic-text` yet (the prototype renders coloured
rects, not text — that's Phase 3 of the roadmap).
