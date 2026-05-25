# Timeline — research + Phase 3.5 prototype

Single session in May 2026 on branch `feat/gpu-terminal`. 12 commits.

## 1. Research (3 commits)

Initial question from the user: tmux scrolling feels rough; Warp
scrolling is smooth. We had a draft GPU terminal spec
(`docs/gpu-terminal-spec.md`) and wanted to know what Warp does that
we don't.

Two research agents ran in parallel against the open-source Warp
repo:

- **Render pipeline & glyph atlas** — confirmed the 3-pipeline /
  instanced-quad architecture, found the Shelf-Next-Fit allocator,
  discovered `MAX_UNUSED_FRAMES = 10` eviction, identified
  `enhance_contrast` and 3-step subpixel positioning as worth
  porting. Flagged our `R8Unorm` atlas as silently breaking emoji.
- **Smooth scroll** — extracted the 7 momentum constants, the
  velocity sampler with `MIN_VELOCITY_TIME_DELTA` floor, the 8 ms
  tick loop via `futures-timer` and `EventLoopProxy`, and the vertex
  shader uniform-shift pattern. Flagged our `scrollback_offset: usize`
  as the root cause of tmux-style jerkiness.

The findings landed as three doc commits on the branch:

| Commit | Summary |
|---|---|
| `4104d4b` | `docs(analysis): add Warp rendering research` (436 lines) |
| `aac5749` | `docs(design): add pixel-based scroll spec with momentum` (433 lines) |
| `d2eaa05` | `docs(gpu-terminal): incorporate Warp findings into spec` (atlas → RGBA8, simpler LRU, subpixel section, scroll uniform, deprecate `usize` scrollback) |

A secondary cleanup landed as commit `5a0ebb5`: the user surfaced an
implicit rule (all docs in English) we then captured as a memory
(`feedback_docs_english_only`) and applied to the spec — translating
~150 Russian lines from earlier drafts.

## 2. Prototype (6 commits)

Decision: rather than start at Roadmap Phase 1 (term_core VT parser),
build the Phase 3.5 smooth-scroll demo first. Reason: it produces an
immediate wow-effect that validates the entire scroll model end-to-end
before any text or VT work is committed.

Location: directly into `crates/term_gpu/` (the real future crate, not
a sandbox).

Six atomic commits, each building on the previous, each compiling
cleanly:

| Commit | Adds |
|---|---|
| `4ff5dff` | `feat(term_gpu): bootstrap crate with hello window` — workspace member, minimal winit ApplicationHandler |
| `0d955fc` | `feat(term_gpu): render colored stripes via wgpu prim pipeline` — instanced quads, 1000 HSV-cycled stripes at offset 0 |
| `ca4664e` | `feat(term_gpu): add pixel-based scroll on wheel input` — `ScrollState { offset_y: f32, ... }`, wheel handler |
| `e5f0bec` | `feat(term_gpu): track scroll velocity for momentum kickoff` — `ScrollVelocity::record` with `MIN_VELOCITY_TIME_DELTA` floor |
| `bb9cf2f` | `feat(term_gpu): add momentum integrator with futures-timer` — full 7-constant integrator via `EventLoopProxy<CustomEvent>` |
| `f8d9d61` | `feat(term_gpu): add sub-pixel ruler overlay for demo` — 1px ticks every 10 scroll-pixels, visible sub-pixel motion |

After the 6th commit, the user ran `cargo run -p term_gpu --example
scroll_demo --release` and confirmed the demo feels good.

## 3. The TouchPhase fix (1 commit)

User report after first run:

> "Fling alone works well, but if I keep scrolling without lifting my
> fingers from the trackpad, scroll and fling start conflicting —
> content jerks back and forth until inertia ends. Warp doesn't have
> this. So we can take the solution from there."

Diagnosis: race between the 50 ms silence timeout and incoming wheel
events. When two wheel events land more than 50 ms apart (which
happens occasionally on macOS), the timeout fires, momentum starts,
the next wheel collides with an in-flight inertia tick.

Fix: `winit 0.30` exposes `TouchPhase` on `MouseWheel` events. On
trackpads, `Phase::Ended` arrives explicitly when fingers physically
lift. Use that for precise (`PixelDelta`) inputs. Keep the silence
timeout as fallback for wheel mice (`LineDelta`) — they never report
`Ended`. `Phase::Cancelled` clears velocity without firing momentum.

Single commit: `baa3d60` — `fix(term_gpu): use TouchPhase::Ended for
trackpad gesture end`. User confirmed jitter gone.

## 4. The lesson encoded for the future

Added to `memory/gpu-terminal-architecture.md` so the rule survives
across sessions:

> Trackpad momentum kickoff uses `TouchPhase::Ended`; wheel mouse uses
> silence timeout. The two have different gesture-end semantics in
> winit and the timeout-only approach causes scroll-fling collisions.

## 5. Phase 3 — Real text rendering (7 commits + 1 chore)

After the user confirmed the prototype was good ("uses crisp scroll"),
we moved to actual text rendering. Six atomic commits + one missed
Cargo.lock chore:

| Commit | Adds |
|---|---|
| `4874d71` | `feat(term_gpu): add GlyphAtlas with RGBA8 texture and shelf packer` — packer, RasterizedGlyph, PlacedGlyph; no cache yet |
| `f103f41` | `feat(term_gpu): integrate cosmic-text and swash rasterizer` — FontSystem, SwashCache, `rasterize_glyph()`; HashMap cache with frame-counter eviction |
| `f2f2dc5` | `chore: update Cargo.lock for cosmic-text 0.14.2` — missed in the previous commit's `git add` |
| `d432712` | `docs(term_gpu): use cosmic-text built-in subpixel positioning` — discovered cosmic-text's `SubpixelBin`; rewrote spec §5.6 and dropped the shader Y-snap |
| `be8d8a9` | `feat(term_gpu): add glyph render pipeline with enhance_contrast` — text.wgsl, GlyphInstance, `create_text_pipeline()` |
| `d3c4c2e` | `feat(term_gpu): wire glyph rendering into GpuRenderer` — atlas+sampler bind group, two-pass single render pass |
| `2b3e12d` | `feat(term_gpu): show shaped text in scroll_demo example` — banner + Lorem ipsum + Row N labels, all with emoji |

## 6. The pushback (4 commits + 1 alignment fix)

User feedback after running the demo:

> "Text is visible but everything is too small, ×5 larger needed.
> Why aren't these optimisations done? Warp is the reference."

Two distinct issues:
- DPI bug (Retina rendered everything at half size)
- Three features I'd labeled "for the prototype" that Warp actually
  ships at parity

I'd violated my own goal of matching Warp. The lesson got encoded as
[memory feedback_no_phase_deferral_for_warp_features.md](../../../memory/)
so I don't repeat it in future sessions.

Four atomic commits in response, plus one alignment hotfix:

| Commit | Fix |
|---|---|
| `03d0363` | `fix(term_gpu): scale geometry and text by window DPI factor` — Uniforms gain `scale_factor`, shaders multiply, instances authored in logical pixels |
| `8615827` | `feat(term_gpu): cache shaped text per (text, font_size, scale_factor)` — `TextShapeCache` keyed on text + style, frame-counter eviction (60 frames) mirroring atlas |
| `5e27c4d` | `feat(term_gpu): cull off-viewport text shaping` — `FrameContext::in_view()` skips ~90 of 100 row labels per frame |
| `47d38e0` | `feat(term_gpu): configure cosmic-text font fallback explicitly` — `FontFamily` enum, `TextShapeCache::with_family()`; doc the automatic fallback chain |
| `0bea2c3` | `fix(term_gpu): match WGSL Uniforms layout to Rust 32-byte struct` — `vec3<f32>` align-16 forced 48-byte struct; replaced with three scalar pads |

User on second run: "everything works great."

## 7. Materials saved for the article

This folder (`docs/articles/warp-gpu-terminal/`) — outline,
research-notes, timeline, quotes-and-numbers — collected immediately
while the work was fresh. Without this, the article would later have
to be rebuilt from commit messages and memory.

## 8. Branch state at end of Phase 3

26 commits on `feat/gpu-terminal`:

- 4 doc/research commits (research, scroll design, spec updates, articles)
- 1 docs translation cleanup
- 1 docs subpixel decision
- 2 Cargo.lock chores
- 6 Phase 3.5 features (smooth scroll prototype)
- 1 fix (TouchPhase trackpad gesture end)
- 6 Phase 3 features (text rendering)
- 4 Phase 3 finishing (DPI, shape cache, culling, font fallback)
- 1 fix (WGSL alignment)

Renderer crate compiles clean, clippy clean, demo runs at 120fps on
Retina with crisp text, momentum, sub-pixel motion, emoji.
