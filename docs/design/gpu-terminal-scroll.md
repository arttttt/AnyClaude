# GPU Terminal — Smooth Scroll Design

> Design doc for pixel-based scrolling with momentum/inertia in the upcoming `term_gpu` crate. Derived from research in [`../analysis/warp-rendering-research.md`](../analysis/warp-rendering-research.md).

## Problem

Our current spec (`docs/gpu-terminal-spec.md`) encodes scroll position as `scrollback_offset: usize` — i.e. a line count. This is the same model used by tmux, and it is the **root cause** of tmux's perceived jerkiness:

- The smallest scroll unit is one full line.
- Trackpad sub-pixel deltas (`0.4`, `0.7` pixels) must be rounded or accumulated, causing visible "stepping."
- There is no provision for momentum / inertia after a swipe ends, so the scroll dies abruptly.

By contrast, Warp scrolls in `f32` pixels, with a 7-constant momentum integrator running off a `winit` event loop and `futures-timer`. Their renderer applies the scroll as a single uniform offset in the vertex shader — no layout recomputation, no atlas churn.

This document specifies the same model for our `wgpu + winit + cosmic-text` stack.

## Goals

1. Sub-pixel-accurate scroll position (`f32` pixels, not lines).
2. Momentum scrolling after a swipe end (trackpad and wheel).
3. Identical feel on macOS / Linux / Windows.
4. No tile cache, no incremental painting — single uniform update + full re-paint of visible area.
5. Frame budget: 120 Hz on capable displays, capped by vsync.

## Non-goals

- Smooth animations for arbitrary UI properties (out of scope; this is just scroll).
- Integration with macOS system momentum (`NSEvent.momentumPhase`) — we run our own integrator, the same approach Warp uses.
- Tile caching of rendered rows — we re-paint visible region each frame, as Warp does.

---

## Architecture

### Data model

```rust
// term_gpu/src/scroll.rs

/// Per-viewport scroll state. Replaces `Grid::scrollback_offset: usize`.
#[derive(Debug, Default)]
pub struct ScrollState {
    /// Pixel offset from the bottom of the scrollback. 0.0 = live tail.
    /// Range: 0.0 ..= total_scrollback_px
    pub offset_y: f32,

    /// Total scrollable height in pixels (sum of all line heights in scrollback + viewport).
    pub total_size_px: f32,

    /// Visible viewport height in pixels.
    pub visible_px: f32,
}

/// Velocity tracker shared with the momentum integrator.
#[derive(Debug, Clone, Copy)]
pub struct ScrollVelocity {
    pub velocity: glam::Vec2,    // pixels per second
    pub last_update: std::time::Instant,
}
```

### Constants (copied verbatim from Warp, MIT)

> Source: [`crates/warpui/src/windowing/winit/event_loop/mod.rs#L58-L73`](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/windowing/winit/event_loop/mod.rs#L58-L73)

```rust
// term_gpu/src/scroll.rs

/// Decay factor applied once per DECAY_INTERVAL (so per second of momentum,
/// velocity multiplies by ~0.018 — near-stop in ~1.5 s).
pub const MOMENTUM_DECAY: f32 = 0.968;

/// Reference interval for the decay factor (8 ms).
pub const MOMENTUM_DECAY_INTERVAL: f32 = 0.008;

/// Momentum tick frequency. 8 ms = ~125 Hz, decoupled from display vsync.
pub const MOMENTUM_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(8);

/// Below this px/s on swipe-end, no momentum kicks in.
pub const MOMENTUM_THRESHOLD: f32 = 50.0;

/// Below this px/s the momentum integrator stops.
pub const MOMENTUM_MIN_VELOCITY: f32 = 1.0;

/// Clamp on initial velocity (defends against batched-event spikes).
pub const MOMENTUM_MAX_VELOCITY: f32 = 2000.0;

/// Floor on time_delta when computing velocity from raw events.
/// Without this, batched winit deliveries (5+ wheel events in one cycle)
/// produce huge velocities. 4 ms floor matches Warp's tuning.
pub const MIN_VELOCITY_TIME_DELTA: f32 = 0.004;

/// Multiplier for non-precise (mouse wheel) LineDelta events.
/// Warp picked 40 over the macOS-reported ~10 to match Chromium/Flutter feel.
pub const NUM_PIXELS_PER_LINE: f32 = 40.0;
```

---

## Input handling

### Winit event branch

```rust
use winit::event::{MouseScrollDelta, WindowEvent};

match event {
    WindowEvent::MouseWheel { delta, .. } => {
        let (precise, raw_delta_px) = match delta {
            MouseScrollDelta::PixelDelta(p) => {
                // Trackpad / Magic Mouse — already in physical pixels.
                let logical = p.to_logical::<f32>(scale_factor);
                (true, glam::vec2(logical.x, logical.y))
            }
            MouseScrollDelta::LineDelta(h, v) => {
                // Wheel mouse — multiply by Warp's empirical constant.
                (false, glam::vec2(h, v) * NUM_PIXELS_PER_LINE)
            }
        };
        self.on_scroll(raw_delta_px, precise, now);
    }
    _ => {}
}
```

Note: `winit` already differentiates trackpad from wheel — we do **not** need AppKit FFI (`hasPreciseScrollingDeltas`). winit reads it under the hood on macOS.

### Velocity tracking

On each scroll event, update the velocity sample:

```rust
fn on_scroll(&mut self, delta_px: glam::Vec2, precise: bool, now: Instant) {
    // 1. Apply delta to position immediately for live feel.
    self.scroll.offset_y = (self.scroll.offset_y + delta_px.y).clamp(
        0.0,
        self.scroll.total_size_px - self.scroll.visible_px,
    );

    // 2. Update velocity sample for potential momentum kickoff.
    let time_delta = self
        .scroll_velocity
        .as_ref()
        .map(|v| now.duration_since(v.last_update).as_secs_f32())
        .unwrap_or(MOMENTUM_DECAY_INTERVAL)
        .max(MIN_VELOCITY_TIME_DELTA);

    self.scroll_velocity = Some(ScrollVelocity {
        velocity: delta_px / time_delta,
        last_update: now,
    });

    self.window.request_redraw();
}
```

### Gesture-end detection (kickoff)

`winit` does not deliver a "trackpad gesture ended" event directly. We use Warp's heuristic: if a precise scroll arrives followed by silence for `~50 ms`, treat that as gesture end and start momentum.

```rust
// In the redraw / tick path:
fn maybe_start_momentum(&mut self, now: Instant) {
    let Some(v) = self.scroll_velocity else { return };
    if now.duration_since(v.last_update) < Duration::from_millis(50) {
        return; // user still scrolling
    }
    let speed = v.velocity.length();
    if speed < MOMENTUM_THRESHOLD {
        self.scroll_velocity = None;
        return;
    }
    let clamped = if speed > MOMENTUM_MAX_VELOCITY {
        v.velocity * (MOMENTUM_MAX_VELOCITY / speed)
    } else {
        v.velocity
    };
    self.start_momentum(clamped, now);
}
```

---

## Momentum integrator

### Timer source

We use `futures-timer` (matches Warp's choice — small, no `tokio` runtime dependency on the rendering side). A new dependency in `term_gpu/Cargo.toml`:

```toml
futures-timer = "3"
futures = "0.3"   # for `futures::future::abortable`
```

### Loop

```rust
use futures::future::{abortable, AbortHandle};
use futures_timer::Delay;
use winit::event_loop::EventLoopProxy;

#[derive(Debug)]
pub enum CustomEvent {
    MomentumTick { window_id: winit::window::WindowId },
}

struct MomentumState {
    abort: AbortHandle,
}

fn start_momentum(
    &mut self,
    initial: glam::Vec2,
    now: Instant,
    proxy: EventLoopProxy<CustomEvent>,
    window_id: winit::window::WindowId,
) {
    self.scroll_velocity = Some(ScrollVelocity { velocity: initial, last_update: now });

    let (fut, abort) = abortable(async move {
        loop {
            Delay::new(MOMENTUM_FRAME_INTERVAL).await;
            if proxy.send_event(CustomEvent::MomentumTick { window_id }).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || futures::executor::block_on(fut));

    self.momentum = Some(MomentumState { abort });
}
```

### Tick handler

Runs on `CustomEvent::MomentumTick`:

```rust
fn on_momentum_tick(&mut self, now: Instant) {
    let Some(v) = self.scroll_velocity.as_mut() else { return };

    let elapsed = now.duration_since(v.last_update).as_secs_f32();
    v.last_update = now;

    // Apply exponential decay over `elapsed` seconds at 8ms base.
    let decay = MOMENTUM_DECAY.powf(elapsed / MOMENTUM_DECAY_INTERVAL);
    v.velocity *= decay;

    if v.velocity.length() < MOMENTUM_MIN_VELOCITY {
        self.cancel_momentum();
        return;
    }

    // Apply position delta.
    let delta = v.velocity * elapsed;
    self.scroll.offset_y = (self.scroll.offset_y + delta.y).clamp(
        0.0,
        self.scroll.total_size_px - self.scroll.visible_px,
    );

    self.window.request_redraw();
}

fn cancel_momentum(&mut self) {
    if let Some(m) = self.momentum.take() {
        m.abort.abort();
    }
    self.scroll_velocity = None;
}
```

### Edge cases

| Case | Behaviour |
|---|---|
| User starts a new scroll while momentum is active | `on_scroll` runs first → updates `scroll.offset_y` and `velocity`. `cancel_momentum()` is called at the top of `on_scroll`. |
| Scroll hits top or bottom | `clamp(0.0, max)` saturates `offset_y`. Velocity continues to decay but produces no visible motion. Could optionally zero velocity at clamp for crisper end. |
| Window loses focus / minimised | `cancel_momentum()` on `Focused(false)`. |
| Display goes to sleep | winit pauses `request_redraw`, no special handling needed. |

---

## Render integration

### Vertex shader: uniform offset

Add `scroll_offset_y: f32` to the per-pass uniforms used by both `text.wgsl` and `prim.wgsl`. The vertex shader subtracts it before NDC conversion.

```wgsl
struct Uniforms {
    screen_size: vec2<f32>,
    scroll_offset: vec2<f32>,   // {0.0, scroll_offset_y}
    _pad: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, in: GlyphInput) -> VsOut {
    let q = QUAD[vi];
    let px = in.pos + q * in.size - uniforms.scroll_offset;
    let ndc = (px / uniforms.screen_size) * 2.0 - 1.0;

    var out: VsOut;
    out.pos = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = mix(in.uv_min, in.uv_max, q);
    out.color = in.color;
    return out;
}
```

This is a **single uniform write per frame**. No vertex/index buffer rebuild. No atlas changes. No layout recomputation.

### CPU-side: viewport selection

The CPU side selects which rows to instance. Convert `scroll.offset_y` to a row range:

```rust
fn visible_row_range(
    rows: &[RowLayout],     // pre-computed pixel Y of each row in scrollback order
    scroll_offset_y: f32,
    visible_px: f32,
) -> std::ops::Range<usize> {
    let top = scroll_offset_y;
    let bottom = scroll_offset_y + visible_px;

    let first = rows.partition_point(|r| r.y_bottom < top);
    let last = rows.partition_point(|r| r.y_top < bottom);

    first..last
}
```

Only rows in `first..last` produce `GlyphInstance` entries. Everything else is culled on CPU.

### Clip rect (Warp pattern)

Wrap the panel paint in a scissor rect so partial rows at top/bottom of viewport are clipped cleanly:

```rust
render_pass.set_scissor_rect(
    panel_rect.x as u32,
    panel_rect.y as u32,
    panel_rect.width as u32,
    panel_rect.height as u32,
);
```

---

## Performance budget

| Operation | Target |
|---|---|
| `on_momentum_tick` | < 50 μs (just velocity math + redraw request) |
| Full frame redraw at 1080p | < 4 ms (leaves headroom for 240 Hz) |
| Uniform upload per frame | 1 × 32-byte write |
| CPU-side culling | O(log N) via `partition_point` on row index |

Effective scroll FPS = `min(125 Hz tick rate, display vsync)`. On a 120 Hz MBP this yields 120 fps during inertia.

---

## Wiring into existing crates

### Changes to `crates/term_core/src/grid.rs`

Deprecate `scrollback_offset: usize`:

```rust
// BEFORE:
pub scrollback_offset: usize,

// AFTER:
/// Pixel offset into scrollback. Replaces `scrollback_offset: usize`.
/// 0.0 = live tail. See docs/design/gpu-terminal-scroll.md.
pub scroll_offset_y: f32,
```

`scrollback_len()` becomes `scrollback_height_px()`. `visible_lines()` is replaced by a `visible_rows(scroll_offset_y, visible_px)` query that returns `&[Line]`.

### Changes to `crates/term_gpu/Cargo.toml`

Add:

```toml
futures = "0.3"
futures-timer = "3"
glam = "0.30"        # if not already present
```

### New file: `crates/term_gpu/src/scroll.rs`

Contains `ScrollState`, `ScrollVelocity`, all constants, `on_scroll`, `start_momentum`, `on_momentum_tick`, `cancel_momentum`.

### Changes to `crates/term_gpu/src/renderer.rs`

- Add `uniforms.scroll_offset` field.
- Wire `CustomEvent::MomentumTick` through `EventLoopProxy`.
- `paint()` reads `scroll.offset_y`, computes row range via `partition_point`, sets scissor.

---

## Test plan

| Test | What it covers |
|---|---|
| Trackpad swipe (precise) | Live `f32` deltas land sub-pixel. |
| Wheel mouse | `LineDelta * 40` produces feel comparable to Chromium. |
| Swipe → release at high velocity | Momentum kicks in; decays smoothly over ~1.5 s. |
| Swipe → release at low velocity | No momentum (velocity < `MOMENTUM_THRESHOLD`). |
| Scroll-then-immediate-scroll | First momentum cancelled before second starts. |
| Scroll to top/bottom | Clamps cleanly, no rubber-band. |
| Focus loss during momentum | Momentum stops. |
| Sustained tick under load | `on_momentum_tick` stays under 50 μs. |

Manual tests on macOS + Linux trackpads, plus a wheel mouse. No automated golden frames yet — feel is subjective.

---

## Open questions

1. **Should we add a soft "rubber-band" overscroll like iOS?** — Out of scope for v1. macOS Cocoa apps usually do, but Warp does not (clean clamp).
2. **Should momentum velocity decay continue while clamped?** — Current spec keeps decaying. Alternative: zero velocity on clamp hit for a crisper end. Decision deferred to first manual test.
3. **Horizontal scroll** — Same model, separate `offset_x`. Out of scope for v1 (terminal content doesn't wrap horizontally in our use case).
4. **Touch / stylus events** — `winit` delivers them as `Touch` events, not `MouseWheel`. Defer; trackpad already covers laptop users.

---

## References

- [`docs/analysis/warp-rendering-research.md`](../analysis/warp-rendering-research.md) — full Warp analysis, §6.
- Warp's [`event_loop/mod.rs`](https://github.com/warpdotdev/warp/blob/main/crates/warpui/src/windowing/winit/event_loop/mod.rs) — momentum logic.
- Warp's [`scrollable.rs`](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/elements/scrollable.rs) — `ScrollData` model.
- Warp's [`clipped_scrollable.rs`](https://github.com/warpdotdev/warp/blob/main/crates/warpui_core/src/elements/clipped_scrollable.rs) — origin shift in paint.
