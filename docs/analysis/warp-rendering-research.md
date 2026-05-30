# Warp Terminal — Rendering & Smooth Scroll Research

> Research conducted May 2026 against the public `warpdotdev/warp` repository (commit `0b737e22a2c75cfef4aa76ac2179112a691bc3b7`). Goal: extract techniques applicable to our custom GPU terminal (stack: `wgpu` + `winit` + `cosmic-text`).

## 0. TL;DR

Warp open-sourced their entire client under **dual MIT/AGPL** in late 2025. The rendering crates (`warpui`, `warpui_core`, `sum_tree`, `editor`) are **MIT** — usable with attribution. The architecture is a near-clone of Zed's `gpui` (shared lineage from the Atom team).

Three findings that change our plan:

1. **Smooth scroll is not Metal magic** — Warp does it with `winit` events, `futures-timer`, and a `f32` pixel offset. It transfers 1:1 to our `wgpu + winit` stack.
2. **Our current spec encodes `scrollback_offset: usize`** (lines, not pixels) — this is the root cause of tmux-style jerkiness. We must move to `f32` pixel offsets.
3. **Three-step subpixel positioning + alpha bitmap glyphs + `enhance_contrast` shader** beats SDF/MSDF for a UI font at a fixed size. We can ship without SDF.

---

## 1. License & legal context

| Path | License | Notes |
|---|---|---|
| `crates/warpui/Cargo.toml` | **MIT** | UI framework — atlas, allocator, renderer, shaders |
| `crates/warpui_core/Cargo.toml` | **MIT** | Scene API, fonts, subpixel |
| `crates/sum_tree/` | MIT | Derived from Zed's sum_tree |
| `crates/editor/` | MIT | Text editing |
| `LICENSE-AGPL` | AGPL-3.0 | Applies to other crates (AI, business logic) |

**Implication:** the rendering stack is safe to **port with attribution**. The AGPL parts (`crates/ai`, `crates/warp_terminal` business logic) we are not touching.

Refs:
- [crates/warpui/Cargo.toml#L6](https://github.com/warpdotdev/warp/blob/main/crates/warpui/Cargo.toml#L6)
- [crates/warpui_core/Cargo.toml#L6](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/Cargo.toml#L6)

---

## 2. Overall architecture: WarpUI (Flutter-inspired)

WarpUI is a custom UI framework with `App / Entity / Handle / View / Element / Scene` — same authors as Atom/Zed, recognisable patterns. Reference: [crates/warpui_core/README.md](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/README.md).

Key abstraction is **`Scene`** — immediate-mode immutable frame description with `Vec<Layer>` containing `Vec<Rect>`, `Vec<Glyph>`, `Vec<Image>`, `Vec<Icon>`. Each frame, `View::render()` rebuilds the `Scene`; then a platform `Renderer` translates it into draw calls.

Refs:
- [crates/warpui_core/src/scene.rs](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/scene.rs)

**For our project:** we do not need the Entity/Handle framework — we have BSP panels with a simpler model. But the **immediate-mode `Scene` rebuild every frame** is a valid pattern, and Warp confirms it scales.

---

## 3. Render backend: split between Metal & wgpu

The most surprising structural decision: **macOS uses Metal directly; Linux/Windows/Wasm use wgpu**. The split is hard-coded in `Cargo.toml` via target-specific dependencies:

- macOS: `metal = "0.33.0"`, `core-text = "21.0.0"`, `cocoa`, `objc`
- Other: `wgpu.workspace = true`, `winit`, `cosmic-text` (warpdotdev fork), `fontdb`, `dwrote` on Windows

There is an experimental feature `experimental-wgpu-renderer` ([Cargo.toml#L30](https://github.com/warpdotdev/warp/blob/main/crates/warpui/Cargo.toml#L30)) — unification attempt for macOS too.

### 3.1 Pipeline count: only three

Both backends expose **exactly three pipelines** — `rect`, `image`, `glyph` — and all three use **instanced rendering over a single 4-vertex + 6-index quad**:

```
quad_vertices = [(0,0),(1,0),(0,1),(1,1)]
quad_indices  = [0,1,2, 2,3,1]
```

Per-instance uniforms are uploaded as a buffer that is re-created each frame (`StorageModeManaged` on Metal).

Refs:
- [crates/warpui/src/rendering/wgpu/renderer.rs#L24-L67](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/wgpu/renderer.rs#L24-L67)
- [crates/warpui/src/platform/mac/rendering/metal/renderer.rs#L130-L175](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/platform/mac/rendering/metal/renderer.rs#L130-L175)
- [crates/warpui/src/platform/mac/rendering/metal/renderer.rs#L181-L196](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/platform/mac/rendering/metal/renderer.rs#L181-L196)

### 3.2 Shaders

- WGSL: [crates/warpui/src/rendering/wgpu/shaders/](https://github.com/warpdotdev/warp/tree/main/crates/warpui/src/rendering/wgpu/shaders) — `rect_shader.wgsl`, `glyph_shader.wgsl`, `image_shader.wgsl`
- Metal: [crates/warpui/src/platform/mac/rendering/metal/shaders/shaders.metal](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/platform/mac/rendering/metal/shaders/shaders.metal) — compiled via `build.rs` to `.metallib` and embedded with `include_bytes!`

Shader-side struct definitions use `bytemuck::Pod + Zeroable` and `wgpu::vertex_attr_array![]` ([shader_types.rs#L87-L100](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/wgpu/shader_types.rs#L87-L100)).

### 3.3 What Warp does NOT use

- **No compute shaders** anywhere in the render path
- **No indirect draws**
- **No bindless textures**

The minimalism is confirmation that our planned 2-pipeline (`text` + `prim`) approach is correct.

### 3.4 Shader techniques worth copying

- **`enhance_contrast(alpha, k)`** in glyph fragment shader — adapted from Windows Terminal's DirectWrite light-text fix. Critical for thin glyphs on dark themes. Ref: [glyph_shader.wgsl#L20-L22](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/wgpu/shaders/glyph_shader.wgsl#L20-L22).
- **SDF rounded rect** (`distance_from_rect`) — standard IQ technique, present in both Metal and WGSL versions.
- **Drop shadow via Gaussian-integral erf approximation** — adapted from a Shadertoy. 4 samples, suitable for overlay/popup borders. Ref: [shaders.metal#L117-L148](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/platform/mac/rendering/metal/shaders/shaders.metal#L117-L148).

---

## 4. Glyph atlas — shelf allocator (matches our `ShelfPacker` plan)

Warp's allocator is in [crates/warpui/src/rendering/atlas/allocator.rs](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/atlas/allocator.rs) and implements **Shelf-Next-Fit** in ~100 lines.

### 4.1 Parameters

| Constant | Value | Ref |
|---|---|---|
| Atlas size | `ATLAS_SIZE = 1024` | [glyph_cache.rs#L24](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/glyph_cache.rs#L24) |
| Horizontal padding | `1 px` | [allocator.rs#L9-L12](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/atlas/allocator.rs#L9-L12) |
| Vertical padding | `1 px` | same |
| Pixel format | **`RGBA8Unorm`** | [metal/renderer.rs#L850-L858](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/platform/mac/rendering/metal/renderer.rs#L850-L858) |

**Important**: Warp uses **`RGBA8Unorm`, not single-channel `R8`** — a single atlas handles both mono glyphs and colour emoji. **Our current spec uses `R8Unorm`, which would silently break emoji.**

### 4.2 Allocator state (3 fields)

```rust
struct ShelfAllocator {
    row_extent: u32,    // right edge of current shelf
    row_baseline: u32,  // top Y of current shelf
    row_tallest: u32,   // max item height on current shelf
}
```

When a rect doesn't fit horizontally: `advance_row` moves `row_baseline += row_tallest + VERTICAL_PADDING`. When the atlas is full, `Manager` allocates a fresh one ([atlas/manager.rs#L47-L66](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/atlas/manager.rs#L47-L66)).

This is **simpler than our current `ShelfPacker`** but logically identical.

### 4.3 Glyph cache key

```rust
GlyphCacheKey {
    glyph_key: GlyphKey,                    // (glyph_id, font_id, OrderedFloat<f32> font_size)
    scale_factor: OrderedFloat<f32>,
    subpixel_alignment: SubpixelAlignment,
}
```

Refs:
- [glyph_cache.rs#L42-L52](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/glyph_cache.rs#L42-L52)
- [scene.rs#L73-L77](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/scene.rs#L73-L77)

### 4.4 Subpixel positioning — 3 steps X, snap Y

```rust
const STEPS: u8 = 3;
let scaled_pos = glyph_position.x().fract() * Self::STEPS as f32;
let alignment = scaled_pos.round() as u8 % Self::STEPS;
```

Each glyph is rasterized **3 times at different horizontal sub-pixel offsets** and cached. In the shader, Y is snapped: `pixel_pos = vec2(pixel_pos.x, floor(pixel_pos.y))`.

Memory cost: `× 3`, visual quality: indistinguishable from continuous subpixel. **Strong technique** that we should adopt.

Refs:
- [fonts.rs#L135-L159](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/fonts.rs#L135-L159)
- [glyph_shader.wgsl#L65](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/wgpu/shaders/glyph_shader.wgsl#L65)

### 4.5 Rasterizer: alpha bitmap, NOT SDF

Standard grayscale-AA via `font-kit`. SDF/MSDF is unused. Contrast is recovered in the fragment shader via `enhance_contrast` — much cheaper than SDF and sufficient for a UI font at a fixed size.

Ref: [fonts/font_kit.rs#L51-L58](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/fonts/font_kit.rs#L51-L58)

**Bonus pattern:** they wrap font-kit calls in an `AutoreleasePoolGuard` on macOS to avoid Objective-C autorelease pool growth across hundreds of glyphs per frame. Ref: [font_kit.rs#L86-L94](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/fonts/font_kit.rs#L86-L94).

### 4.6 Eviction: frame counter, not LRU

`TextureCache::end_frame` increments a counter on each cached glyph; anything unused for `MAX_UNUSED_FRAMES = 10` is dropped.

Ref: [texture_cache.rs#L48-L71](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/rendering/texture_cache.rs#L48-L71)

**Implication for our plan:** the doubly-linked-list LRU in our spec is overkill. A `last_used_frame: u32` field on each cached glyph is enough.

---

## 5. Font shaping & text layout

### 5.1 macOS: Core Text directly

Warp uses **CTFramesetter / CTLine** via FFI. They build `CFMutableAttributedString` with per-run attributes (font, color, kerning, paragraph style), and `CTFramesetter` produces a layout from which `glyphs()`, `positions()`, `string_indices()`, `advances()` are extracted.

Ref: [platform/mac/text_layout.rs#L539-L580](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/platform/mac/text_layout.rs#L539-L580)

### 5.2 Linux/Windows: cosmic-text (forked)

```toml
cosmic-text = { git = "https://github.com/warpdotdev/cosmic-text.git", rev = "15198beba692162201c0ea8b15222cf5643ea068" }
```

Ref: [Cargo.toml#L78](https://github.com/warpdotdev/warp/blob/main/crates/warpui/Cargo.toml#L78)

They use a **forked** cosmic-text plus `fontdb = "0.23.0"` for font discovery. We can start with upstream cosmic-text; if their patches matter we can investigate the diff later.

### 5.3 Shaped-run caching

`crates/warpui_core/src/text_layout.rs` has a `LayoutCache` (RWLock'd HashMap) keyed by `LayoutCacheKey`. We need the same primitive.

---

## 6. Smooth scroll — the core of "feel"

This is the answer to "why does Warp scroll feel like silk and tmux feel like sandpaper."

### 6.1 Files involved

| File | Role |
|---|---|
| `crates/warpui/src/windowing/winit/event_loop/mod.rs` (2058 lines) | main winit handler, momentum timer |
| `crates/warpui/src/windowing/winit/app.rs` | `CustomEvent::MomentumScroll` |
| `crates/warpui_core/src/elements/scrollable.rs` | Scrollable wrapper |
| `crates/warpui_core/src/elements/clipped_scrollable.rs` | sub-pixel viewport translation |
| `crates/warpui_core/src/elements/new_scrollable/mod.rs` | newer implementation |
| `crates/warpui/src/platform/mac/event.rs` | legacy Cocoa fallback |

### 6.2 Pixel-based always

```rust
pub struct ScrollData {
    pub scroll_start: Pixels,   // f32 wrapper — like CSS scrollTop
    pub visible_px: Pixels,
    pub total_size: Pixels,
}
```

Ref: [scrollable.rs#L60-L75](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/elements/scrollable.rs#L60-L75)

**No "lines"**. Scroll is in `f32` pixels. A 0.5-pixel trackpad delta lands as 0.5 pixels with no rounding.

### 6.3 Raw input handling via winit

```rust
WindowEvent::MouseWheel { delta, .. } => {
    let (precise, delta) = match delta {
        MouseScrollDelta::LineDelta(h, v) => (false, Vector2F::new(h, v)),
        MouseScrollDelta::PixelDelta(px) => {
            (true, px.to_logical(scale_factor).to_vec2f())
        }
    };
    // ...
}
```

Ref: [event_loop/mod.rs#L1241-L1256](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/windowing/winit/event_loop/mod.rs#L1241-L1256)

**winit already differentiates trackpad (`PixelDelta`) from mouse wheel (`LineDelta`)** — we do not need direct AppKit FFI. The `precise: bool` tag is propagated downstream.

Wheel mouse → multiply by `NUM_PIXELS_PER_LINE = 40.0`:

```rust
if precise {
    self.child.scroll(delta.into_pixels(), ctx);
} else {
    self.child.scroll(delta * 40.0, ctx);
}
```

Ref: [scrollable.rs#L281-L301](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/elements/scrollable.rs#L281-L301)

Warp picked `40.0` to match Chromium/Flutter — `CGEventSourceGetPixelsPerLine` returns ~10, which feels too slow.

### 6.4 Momentum scrolling — all the constants

```rust
const MOMENTUM_DECAY: f32 = 0.968;             // decay factor per interval
const MOMENTUM_DECAY_INTERVAL: f32 = 0.008;    // 8ms reference interval
const MOMENTUM_FRAME_INTERVAL: Duration = Duration::from_millis(8); // ~125 Hz tick
const MOMENTUM_THRESHOLD: f32 = 50.0;          // min velocity to start inertia
const MOMENTUM_MIN_VELOCITY: f32 = 1.0;        // sub-pixel — stop
const MOMENTUM_MAX_VELOCITY: f32 = 2000.0;     // clamp against spikes
const MIN_VELOCITY_TIME_DELTA: f32 = 0.004;    // floor for time delta (batched events)
```

Ref: [event_loop/mod.rs#L58-L73](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/windowing/winit/event_loop/mod.rs#L58-L73)

These are **empirically tuned** — copy verbatim.

### 6.5 Velocity tracking

```rust
struct ScrollVelocity {
    velocity: Vector2F,
    last_update: Instant,
}
```

Updated on each move:

```rust
let time_delta = now.duration_since(v.last_update)
    .as_secs_f32()
    .max(MIN_VELOCITY_TIME_DELTA);
window_state.scroll_velocity = Some(ScrollVelocity {
    velocity: delta / time_delta,
    last_update: now,
});
```

Refs: [event_loop/mod.rs#L118-L123](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/windowing/winit/event_loop/mod.rs#L118-L123), [event_loop/mod.rs#L364-L394](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/windowing/winit/event_loop/mod.rs#L364-L394).

**Critical**: `MIN_VELOCITY_TIME_DELTA = 0.004` (4 ms floor) protects against batched events. Without it, when winit delivers 5 wheel events in one cycle, time_delta can be ~0 and velocity explodes.

### 6.6 Momentum timer

On gesture end with `velocity.length() >= MOMENTUM_THRESHOLD`, an abortable timer fires every 8 ms:

```rust
let (future, abort_handle) = futures::future::abortable(async move {
    loop {
        Timer::after(MOMENTUM_FRAME_INTERVAL).await;
        let _ = proxy.send_event(CustomEvent::MomentumScroll { window_id });
    }
});
```

Ref: [event_loop/mod.rs#L1746-L1767](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/windowing/winit/event_loop/mod.rs#L1746-L1767)

Main decay loop:

```rust
let elapsed = now.duration_since(v.last_update).as_secs_f32();
v.last_update = now;
let decay = MOMENTUM_DECAY.powf(elapsed / MOMENTUM_DECAY_INTERVAL);
v.velocity *= decay;
if v.velocity.length() < MOMENTUM_MIN_VELOCITY {
    cancel_momentum_scroll();
    return;
}
let delta = v.velocity * elapsed;
// emit synthetic ScrollWheel event
```

Ref: [event_loop/mod.rs#L795-L829](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/windowing/winit/event_loop/mod.rs#L795-L829)

**They ignore macOS system momentum** (`NSEvent.momentumPhase`). Their own inertia integrator runs cross-platform — identical feel on macOS / Linux / Windows.

### 6.7 Applying scroll in paint — the trick

```rust
fn paint_internal(&mut self, origin: Vector2F, ctx: &mut PaintContext) {
    ctx.scene.start_layer(ClipBounds::BoundedBy(RectF::new(origin, size)));
    self.child.paint(
        origin - self.state.scroll_start().as_f32().along(self.axis),
        ctx,
    );
    ctx.scene.stop_layer();
}
```

Ref: [clipped_scrollable.rs#L298-L313](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/elements/clipped_scrollable.rs#L298-L313)

**This is the whole magic**: shift the child's origin by `-scroll_start` (a `Vector2F` float) and wrap in a clip layer. No layout recomputation; layout was already done. Just change one `f32` and re-paint.

**Implication for our pipeline**: this is a single uniform update in the vertex shader. Nothing in the atlas changes; nothing in the layout cache changes.

### 6.8 No tile cache, no incremental rendering

Warp fully re-paints the viewport every frame. There is **no row tile cache** in their scroll path. They rely on the wgpu pipeline being fast enough — and it is.

We should not over-engineer with tile caches in our renderer.

### 6.9 Redraw cadence

`MOMENTUM_FRAME_INTERVAL = 8ms` timer → emit `CustomEvent::MomentumScroll` → update `scroll_start` → `window.request_redraw()` → winit's `RedrawRequested` → vsync caps the effective FPS.

**Effective FPS = min(8ms tick = 125 Hz, display vsync)**. No `CADisplayLink` / `CVDisplayLink`. Standard `winit::request_redraw + vsync`.

### 6.10 Trackpad vs mouse

`hasPreciseScrollingDeltas` is only read in their **legacy Cocoa path** ([platform/mac/event.rs#L225-L240](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/platform/mac/event.rs#L225-L240)). The main path **trusts winit's `PixelDelta` vs `LineDelta`** — winit reads `hasPreciseScrollingDeltas` under the hood.

---

## 7. What to copy / what not to copy

### High priority — copy directly (MIT, attribute)

| Item | Source | Why |
|---|---|---|
| Shelf-Next-Fit allocator | [atlas/allocator.rs](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/atlas/allocator.rs) | 100 lines, complete |
| 3-step subpixel + snap Y | [fonts.rs SubpixelAlignment](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/fonts.rs#L135-L159) | strong quality / memory ratio |
| `GlyphCacheKey` triple | [glyph_cache.rs#L42-L52](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/glyph_cache.rs#L42-L52) | obvious hashmap key |
| Momentum scroll (all 7 constants) | [event_loop/mod.rs#L58-L73](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/windowing/winit/event_loop/mod.rs#L58-L73) | tuned empirically |
| `enhance_contrast(alpha, k)` | [glyph_shader.wgsl#L20-L22](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/rendering/wgpu/shaders/glyph_shader.wgsl#L20-L22) | thin text on dark themes |
| `distance_from_rect` SDF | shaders | rounded panel borders |
| Drop shadow via erf, 4 samples | [shaders.metal#L117-L148](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/platform/mac/rendering/metal/shaders/shaders.metal#L117-L148) | popup/menu shadows |
| `MAX_UNUSED_FRAMES = 10` eviction | [texture_cache.rs#L48-L71](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/rendering/texture_cache.rs#L48-L71) | replaces full LRU |
| Instanced quad pattern | [metal/renderer.rs#L181-L196](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/platform/mac/rendering/metal/renderer.rs#L181-L196) | confirms our plan |
| `AutoreleasePoolGuard` | [font_kit.rs#L86-L94](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/fonts/font_kit.rs#L86-L94) | macOS memory hygiene |

### Confirms our existing plan (no change needed)

- 2-3 pipelines (rect + glyph; image is optional)
- Alpha bitmap glyphs, not SDF
- Instanced quads (6 vertices, 4 unique)
- WGSL shaders
- cosmic-text for shaping (upstream is fine)

### Do NOT copy

- **sum_tree** — overkill for a VT cell grid. We have `Vec<Row>` already.
- **Entity/Handle/Scene framework** — full `gpui` clone, we have BSP panels.
- **AppKit FFI in `platform/mac/event.rs`** — legacy path, replaced by winit.
- **`CADisplayLink` / `CVDisplayLink`** — not used by Warp either.
- **Core Text path** — only worth it if we were macOS-only with rich typography.
- **`cosmic-text` fork** — start from upstream; investigate the diff later if needed.

---

## 8. Architecture comparison: our spec vs Warp

| Aspect | Our spec (`docs/gpu-terminal-spec.md`) | Warp | Action |
|---|---|---|---|
| Atlas allocator | `ShelfPacker` (DIY) | Shelf-Next-Fit (3 fields) | match — port their version |
| Atlas format | `R8Unorm` | `RGBA8Unorm` | **change to RGBA8** — emoji |
| Glyph cache eviction | Doubly-linked LRU | `MAX_UNUSED_FRAMES = 10` counter | **simplify** |
| Subpixel positioning | not specified | 3 steps X, snap Y | **add** |
| `enhance_contrast` in shader | not present | yes | **add** |
| Pipelines | 2 (text + prim) | 3 (rect + image + glyph) | OK, image optional |
| Vertices per quad | 6 (2 tris) | 6 (2 tris, 4 unique + indices) | match |
| Scroll model | `scrollback_offset: usize` (lines!) | `scroll_start: f32` (pixels) | **rewrite as pixel-based** |
| Momentum / inertia | not specified | 7 constants + timer | **add — see new doc** |
| Origin shift in paint | not present | uniform shift, full repaint | **add** |
| Drop shadow shader | not present | erf-based, 4 samples | add for overlays |
| Compute shaders | none | none | match |
| Indirect draws | none | none | match |
| Font shaping | cosmic-text | cosmic-text fork | use upstream |
| Text model | `Vec<Row>` of `TextRun` | sum_tree | OK — `Vec<Row>` is right for us |
| Window/event loop | (TBD) | winit | match |
| Display refresh | (TBD) | `winit::request_redraw` + vsync | match |

---

## 9. Bottom-line items added to the plan

1. **`docs/design/gpu-terminal-scroll.md`** — pixel-based scroll, momentum, origin shift.
2. **Update `docs/gpu-terminal-spec.md`** — `RGBA8`, simpler LRU, subpixel section, `enhance_contrast`, deprecate `scrollback_offset: usize`.
3. **`memory/gpu-terminal-architecture.md`** — restore the broken pointer with key decisions.

All Warp code we port lands with `// Adapted from warpdotdev/warp (MIT)` and a file:line reference in the comment.
