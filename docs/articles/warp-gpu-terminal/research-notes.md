# Research notes — Warp's rendering stack & smooth scroll

Style is closer to the article than the source — drop-in candidates
for the published piece. Full background and file:line references are
in [`docs/analysis/warp-rendering-research.md`](../../analysis/warp-rendering-research.md);
exact constants are in [`quotes-and-numbers.md`](quotes-and-numbers.md).

---

## 1. What surprised us

Warp open-sourced the entire client, not just a token piece. The
rendering crates (`warpui`, `warpui_core`, `sum_tree`) are MIT;
business logic (`crates/ai`, `crates/warp_terminal`) is AGPL. So the
parts a third party would actually want to port — atlas, shelf
allocator, shaders, scroll integrator — are reusable with attribution.

The architecture is a near-clone of Zed's `gpui`. Once you've seen
`Scene` / `Layer` / `Element` / `View` in either, you recognise it
instantly. The Atom-Zed-Warp lineage is visible in the code.

The whole renderer is **three pipelines**: `rect`, `image`, `glyph`.
All instanced quads over a 4-vertex / 6-index unit. **No compute
shaders. No indirect draws.** That's it. The minimalism is its own
result — it tells you what you don't need.

## 2. Glyph atlas — pragmatism beats sophistication

- `RGBA8Unorm`, not single-channel `R8`. One texture serves mono
  glyphs (in the alpha channel) and colour emoji. Our first draft of
  the spec had `R8` — would have silently broken emoji.
- The allocator is ~100 lines of Shelf-Next-Fit. Three state fields
  (`row_baseline`, `row_extent`, `row_tallest`). When a row fills up,
  advance to the next shelf. When the atlas fills up, allocate a new
  one. That's the whole algorithm.
- Eviction is a per-glyph `last_used_frame` counter. Drop anything
  unused for 10 consecutive frames. Beats an intrusive LRU at the
  cost of zero accuracy that matters here.
- Subpixel positioning: Warp rasterizes each glyph at 3 horizontal
  offsets and snaps Y with `floor(px.y)`. Memory cost ×3 per glyph;
  quality indistinguishable from continuous subpixel. We **planned**
  to adopt this verbatim, but during the prototype we discovered
  cosmic-text already ships built-in `SubpixelBin` (4×4 bins per glyph)
  via `CacheKey`. Using the built-in costs us ×16 memory per glyph
  variant instead of ×3, in exchange for zero hand-rolled code and
  crisper Y positioning. Worth flagging in the article as a "read
  the docs before reimplementing the cool thing" moment.

## 3. Shaders — small tricks that punch above their weight

- `enhance_contrast(alpha, k)` lifts the perceived weight of thin
  glyphs on dark themes. Adapted from Windows Terminal's DirectWrite
  shader. Two lines of WGSL.
- `distance_from_rect()` for rounded panel borders. Standard IQ SDF.
- Drop shadow via a 4-sample Gaussian-integral `erf` approximation.
  Adapted from a Shadertoy. Cheap enough to use freely on
  popups/overlays.

None of these are exotic. They're all worth porting because they each
fix a class of visible defect the user notices.

## 4. The font-shaping split

- macOS: Core Text directly (`CTFramesetter`, `CTLine`).
- Linux / Windows / Wasm: a **forked** `cosmic-text`
  (`warpdotdev/cosmic-text` at commit `15198beba`).
- Both paths feed the same shelf allocator.

We picked upstream `cosmic-text`. If their fork turns out to fix
something we care about, we can investigate the diff later.

## 5. Smooth scroll — the answer is mundane

The number-one feature people associate with Warp is scroll feel. It's
not Metal magic. It's:

- A pixel-precise `f32` offset (not a line count).
- A velocity sampler with a 4 ms `time_delta` floor (defends against
  batched wheel events that would otherwise produce ~0 deltas and
  explode the velocity).
- An 8 ms tick loop emitted via `EventLoopProxy<MomentumTick>`,
  running on `futures-timer` (no tokio runtime needed on the renderer
  side).
- Exponential decay each tick: `velocity *= 0.968 ^ (elapsed / 8ms)`.
- On the GPU side, **one uniform**: `scroll_offset: vec2<f32>`
  subtracted before the NDC transform. No layout recompute, no atlas
  change, no tile cache. Single uniform write per frame.

The seven constants (`MOMENTUM_DECAY=0.968` etc., listed in
`quotes-and-numbers.md`) are empirically tuned. The hard part is
knowing they exist — not deriving them.

## 6. Cross-platform feel without AppKit FFI

Warp does **not** use `NSScrollView`, `CADisplayLink`, `CVDisplayLink`,
or `hasPreciseScrollingDeltas`. It uses `winit` like everyone else.

`winit` already differentiates trackpad from wheel mouse via
`MouseScrollDelta::PixelDelta` vs `LineDelta`. On macOS, `winit` reads
`hasPreciseScrollingDeltas` under the hood.

For gesture-end detection, `winit 0.30` reports `TouchPhase::Ended` on
trackpad lift. We learned this the hard way — see "The TouchPhase
fix" below.

The pay-off: identical scroll feel on macOS, Linux, and Windows from a
single code path.

## 7. The TouchPhase fix

First implementation: detect end-of-gesture by silence (no wheel event
for 50 ms → fire `GestureEnded` via `EventLoopProxy`).

Symptom: clean swipe-and-fling worked. But continuous scroll without
lifting fingers from the trackpad caused content to jitter — small
back-and-forth jumps.

Diagnosis: between two wheel events delivered by macOS, sometimes >50
ms passes. The timeout fires. Momentum starts. The next wheel event
arrives a few ms later, collides with an in-flight inertia tick, and
the content visibly snaps.

Fix: precise (`PixelDelta`) inputs come with explicit `TouchPhase`.
Use `TouchPhase::Ended` as the authoritative gesture-end signal for
trackpads. Keep the 50 ms silence timeout as fallback for wheel mice,
which never report `Ended`.

Single commit. Bug gone.

## 8. What we deliberately did NOT copy

- **`sum_tree` text model.** Warp uses it (inherited from Zed) for
  rope-style editing. Overkill for a VT cell grid. `Vec<Row>` is the
  right choice for our use case — Claude Code is an ink-based TUI, not
  a code editor.
- **Entity / Handle / Scene framework.** A full `gpui` clone. We have
  BSP panels and don't need the abstraction.
- **AppKit FFI in `crates/warpui/src/platform/mac/event.rs`** — a
  legacy path Warp itself has moved off of.
- **The `cosmic-text` fork.** Start with upstream; investigate later if
  needed.
- **Compute shaders, indirect draws, bindless textures, tile caches,
  SDF / MSDF glyphs.** None of these earn their complexity for a
  fixed-size UI font at terminal scale.

## 9. The minimal architecture we ended up with

- 3 custom crates: `term_core` (VT parser), `term_gpu` (renderer +
  atlas + scroll), `term_layout` (BSP panels).
- 6 external deps: `wgpu`, `winit`, `cosmic-text`, `futures`,
  `futures-timer`, `glam`.
- 2 render pipelines: `rect` and `glyph`. (Image is optional, planned
  for later.)
- `Vec<Row>` of `TextRun` for the cell grid. No `sum_tree`.
- Pixel-based scroll with Warp's 7-constant momentum integrator.
- `RGBA8Unorm` glyph atlas with frame-counter eviction and 3-step
  subpixel positioning.

Everything else (selection, scrollback navigation, font fallback,
drop-shadow on overlays) is polish.
