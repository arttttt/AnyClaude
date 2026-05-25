# Outline — "How Warp scrolls: building a smooth GPU terminal in a weekend"

> Working title. The article is not yet written — this is the structure
> and key beats. See sibling files for substance, timeline, and quotes.

## Hook

The user starts with a complaint: tmux scrolling feels rough; Warp
scrolling feels like silk. We're building a custom Rust GPU terminal
for the AnyClaude wrapper. Can we get Warp-quality scroll by reading
Warp's source?

Warp open-sourced their client under dual MIT/AGPL in late 2025. The
rendering stack is MIT. We can.

## Act 1 — What Warp actually is

- Warp is a custom UI framework (`warpui`, `warpui_core`) with a
  Flutter-inspired `Scene` / `Element` / `View` model. Same authors as
  Atom and Zed — recognisable patterns.
- Two render backends: Metal directly on macOS, `wgpu` everywhere else
  (with an experimental wgpu path on macOS too).
- The whole pipeline is 3 shaders: `rect`, `image`, `glyph`. All draw
  calls are instanced quads over a 4-vertex / 6-index unit quad. No
  compute shaders. No indirect draws. No tile cache.
- Glyph atlas is `RGBA8Unorm` (so emoji and monochrome glyphs share a
  texture). Shelf-Next-Fit packer in ~100 lines. Eviction is a
  per-glyph frame counter (`MAX_UNUSED_FRAMES = 10`), not an LRU.
- Subpixel positioning: 3 horizontal variants per glyph, snap Y in the
  vertex shader. `×3` memory, no visible artifacts.
- Text shaping: Core Text on macOS, a forked `cosmic-text` elsewhere.

These are the foundations. They will recur in our article as design
patterns the reader can take to other projects.

## Act 2 — Why tmux feels rough (the root cause)

- tmux's scroll model is `scrollback_offset: usize` — a line count.
  Smallest unit is one row. Sub-pixel trackpad deltas have nowhere to go.
- Our first draft of the GPU terminal spec inherited the same model.
  That would have shipped the same jitter.

Concrete contrast:

| Aspect            | tmux / our v1 draft     | Warp / our v2          |
|-------------------|--------------------------|------------------------|
| Scroll unit       | One row (usize)          | One pixel (f32)        |
| Trackpad deltas   | Rounded / discarded      | Applied as-is          |
| Inertia           | None                     | Custom integrator      |
| Cross-platform    | Whatever the terminal does | Identical on all OS  |

## Act 3 — Warp's smooth-scroll secret

- It's not Metal magic. It's `winit` + `futures-timer` + 7 numeric
  constants empirically tuned (`MOMENTUM_DECAY=0.968`,
  `MOMENTUM_DECAY_INTERVAL=8ms`, `MOMENTUM_THRESHOLD=50`,
  `MOMENTUM_MIN_VELOCITY=1`, `MOMENTUM_MAX_VELOCITY=2000`,
  `MIN_VELOCITY_TIME_DELTA=4ms`, `NUM_PIXELS_PER_LINE=40`).
- Velocity is `Vec2`, sampled on every wheel event with a 4 ms floor
  on `time_delta` (defends against batched event spikes).
- Momentum is a separate 8 ms tick loop emitted via `EventLoopProxy`,
  decayed exponentially, stopped when velocity drops below 1 px/s.
- The render-side change is a single `scroll_offset: vec2<f32>` uniform
  subtracted in the vertex shader. No layout recompute. No atlas
  change. No tile cache.

## Act 4 — Building the prototype

- ~11 commits in a single branch, single afternoon.
- 3 new docs (research, scroll design, spec update).
- 6 atomic feature commits: bootstrap → stripes → pixel scroll →
  velocity → momentum → ruler overlay.
- The prototype reaches "feels like Warp" with `cargo run -p term_gpu
  --example scroll_demo --release`.

## Act 5 — The one gotcha that needed a fix

- First version: a 50 ms silence timeout to detect gesture end.
- User report: "fling alone is fine; but if I keep scrolling without
  lifting my fingers, the content jitters back and forth."
- Diagnosis: a long gap between two wheel events fires the timeout,
  kicks off momentum, then the next wheel event collides with an
  in-flight inertia tick.
- Fix: trackpads in `winit 0.30` deliver `TouchPhase::Ended` when
  fingers physically lift. Use that as the authoritative gesture-end
  signal for precise (`PixelDelta`) input. Keep the silence timeout as
  a fallback for wheel mice (`LineDelta`) which never report `Ended`.
- One commit. Bug gone.

## Act 6 — Adding real text rendering

Once scroll feel was nailed, we built the actual text pipeline:

- `RGBA8` glyph atlas with a 100-line Shelf-Next-Fit packer ported
  from Warp.
- `cosmic-text` for shaping (upstream, not Warp's fork).
- A second wgpu pipeline (`text.wgsl`) reusing the same instanced quad
  pattern, plus an atlas texture+sampler bind group at `@group(1)`.
- A subtle decision: cosmic-text already does subpixel positioning
  via `CacheKey::SubpixelBin` (4 X-bins × 4 Y-bins = 16 variants per
  glyph). We had planned to port Warp's hand-rolled `SubpixelAlignment`
  (3 X-bins + Y-snap in the shader), but the library version costs us
  zero hand-rolled code in exchange for ×16 vs ×3 memory per glyph.
  Worth flagging as a "read the docs before reimplementing the cool
  thing" moment.

## Act 7 — "Why isn't this implemented?"

Demo shipped. First user reaction:

> "Text is visible but everything is too small — about ×5 larger needed.
> And why aren't these optimizations done? Warp does them."

Two distinct mistakes:

1. **DPI bug.** Our coordinate system treated logical pixels as
   physical, halving everything on Retina. Fix: move scale_factor into
   the Uniforms struct, multiply in the vertex shader, author all
   instance data in logical pixels.

2. **The "for the prototype" trap.** I'd labeled shape caching, CPU
   culling, and font fallback configuration as "polish for later" when
   Warp ships all three at parity. User correctly pushed back: this is
   the real implementation, not a prototype.

The lesson got encoded as a memory rule
([feedback_no_phase_deferral_for_warp_features.md](../../../memory/)):
when implementing GPU terminal pieces, anything Warp does in the
comparable area belongs in the current phase. No "I'll catch up later"
deferrals.

What got built in response:

- DPI uniform + shader scaling
- `TextShapeCache` keyed by `(text, font_size, scale_factor,
  wrap_width)` with frame-counter eviction mirroring the atlas
- CPU viewport culling via a `FrameContext::in_view()` predicate —
  on a 720 px viewport, 90 of 100 row labels get skipped per frame
- `FontFamily` enum + `TextShapeCache::with_family()` for explicit
  primary family choice; emoji and CJK fallback automatic via
  cosmic-text/fontdb

## Act 8 — One more gotcha: WGSL alignment

Release build failed at first draw:

> `Buffer is bound with size 32 where the shader expects 48 in
> group[0] compact index 0`

Root cause: `vec3<f32>` in WGSL has **alignment 16** (not 12). Putting
a `vec3` pad at the end of a uniform struct rounds the struct size up
to 48 bytes — but our hand-written Rust `Uniforms` was 32 bytes.

Fix: replace `_pad: vec3<f32>` with three scalar `f32` pads in both
shaders. Scalar f32 has align 4, so the struct stays 32 bytes.

Worth a callout in the article: WGSL/std140 alignment rules are
non-obvious, and writing uniforms by hand (without `bytemuck` or
`encase`) means owning that rulebook yourself.

## Outro — Takeaways

1. **Read the source.** Warp's MIT crates contain answers to questions
   that would take weeks of research otherwise.
2. **Pixel-based scroll is non-negotiable** for "feels smooth".
3. **The whole momentum integrator is 7 constants and a timer.** Not
   exotic. The hard part is knowing the constants exist.
4. **Trust winit for trackpad detection** — `PixelDelta` vs `LineDelta`
   and `TouchPhase::Ended` already encode what you need on every
   platform. Skip the AppKit FFI temptation.
5. **A 100-line shelf allocator is enough** for a glyph atlas. SDF /
   MSDF / compute shaders are over-engineering for a fixed-size UI font.
6. **Use libraries' built-in subpixel before rolling your own.**
   cosmic-text bins to 16 variants per glyph at zero code cost.
7. **DPI awareness lives in the uniform, not the call site.** Author
   instance data in logical pixels; multiply by `scale_factor` in the
   vertex shader. One field, one multiplication, no bug-prone scattered
   `* dpi` calls.
8. **`vec3` in a WGSL uniform aligns to 16.** If you write uniforms by
   hand, use scalar pads or `vec4`. The validator catches it
   immediately but the error message is cryptic.
9. **Don't defer features your reference implementation ships.**
   "I'll add caching later" creates technical debt that compounds
   across phases. Build the real thing each phase.

## Possible follow-up articles

- "Building the term_core VT parser: what Claude Code actually emits"
- "From 60 to 240 FPS: profiling a wgpu terminal"
- "Why we chose `Vec<Row>` over `sum_tree` for a terminal grid"
