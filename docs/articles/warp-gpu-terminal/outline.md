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

## Act 9 — Phase 1: the actual VT parser

With rendering working, time to handle real ANSI bytes. Two big
decisions before any code, both made after a second research pass
against Warp:

1. **Hand-roll the Paul Williams state machine, no `vte` crate.**
   Warp uses (a fork of) `vte`. We don't, to keep `term_core`
   dependency-free. Trade-off: ~770 LoC of careful state-transition
   code. Worth it for the simplicity.

2. **Fixed-cell logical grid (alacritty-style), variable-width
   render.** ink (Claude Code's TUI framework) assumes a monospace
   cell grid for cursor positioning — `CUP row 5 col 10` must
   address one definite cell. Our earlier "variable-width spans"
   plan would have broken VT semantics. Variable-width happens in
   `term_gpu` at shape time. Logical model is monospace; visual
   model isn't.

The implementation: 8 atomic commits. Highlights:

- `Cell { c, fg, bg, flags, extra: Option<Box<CellExtra>> }` with
  heap-indirected metadata for rare features (combining marks,
  hyperlinks, prompt markers).
- 30+ `Action` variants covering all P0 + P1 sequences from the
  research priority list — including the ones our first spec missed:
  ICH, DCH, ECH, REP, VPA, CNL/CPL, DA (must reply, apps hang
  otherwise), DECSCUSR cursor style, OSC 7 CWD, OSC 8 hyperlinks,
  OSC 133 prompt markers, DEC 1004 focus reporting, DECSET 2026
  sync output.
- OSC 8 stickiness model: `Grid.current_hyperlink` is `Some(...)`
  until an empty-URL OSC 8 closes it; `Grid.print` attaches it to
  every cell while active.
- OSC 133 one-shot model: `Grid.next_prompt` is a tag attached to
  the next printed cell and cleared. Different semantics from OSC 8.
- 39 integration tests in `crates/term_core/tests/` (parser_smoke +
  emulator_smoke). Per project policy, no `#[cfg(test)]` in `src/`.
- `examples/dump.rs`: pipe raw ANSI into stdin, get a framed ASCII
  grid plus cursor/title/cwd state out. Useful for replaying real
  Claude Code captures.

## Act 10 — Wiring it up: term_core × term_gpu

With both crates working in isolation, the next question was
whether they actually fit together. A new example
(`crates/term_gpu/examples/render_term.rs`) pipes raw ANSI bytes
from stdin through `VtEmulator` into the GPU window. The interesting
part isn't the plumbing — it's what surfaced when real `term_core`
output hit the GPU stack:

1. **The blur was DPI, not subpixel.** First user feedback was
   "тексты слегка размыты". We tried pixel-snapping cell origins,
   then per-cell shaping, then Y-snapping the baseline — each fix
   *slightly* better, never crisp. Real cause: a YAGNI regression
   from commit 1. I'd removed `self.scale_factor =
   renderer.scale_factor()` because nothing in *that* commit consumed
   it. By commit 2 the shape calls used `self.scale_factor` (still
   `1.0`) while the framebuffer was Retina — glyphs rasterised at
   logical-pixel size and the GPU sampler bilinearly stretched them
   ×2. Single field restore made the text crisp.

2. **Warp doesn't use shaper advances for positioning.** A second
   delegated-agent research pass against Warp's actual
   `render_cell_glyph` / `paint_line` revealed: even when shaping is
   engaged (ligatures, combining marks), Warp **discards the shaper's
   advances** and places each glyph at `col × cell_width`. The
   `cell_width` is `round(advance_of('m')).max(1)` — integer
   physical pixels. Adopting this pattern fixed our remaining
   alignment drift. Lesson: a per-cell snap on cell origin is not
   enough if the shaper itself returns fractional advances; you have
   to **ignore the advances**.

3. **`Grid::resize` infinite-looped on first window open.** The loop
   bound `self.visible_start() + rows` depended on `rows.len() -
   self.visible_rows` (still old) and grew in lock-step with each
   push. Classic "loop invariant mutated by loop body" bug.

4. **Top-anchored resize is non-default but user-chosen.**
   Standard terminal behaviour (alacritty / xterm) scrolls top rows
   into scrollback on shrink and pulls them back on grow. The user
   explicitly wanted "контент resizes но не двигается куда-либо"
   (their Warp config). Re-wrote `Grid::resize` as truncate-bottom
   on shrink + pad-bottom on grow + cursor clamp, leaving the
   alacritty-style algorithm as a possible future flag.

## Act 11 — Phase 4: BSP panels

`term_layout` is the smallest of the three crates by far —
~250 LoC of recursive `Box<Node>` BSP. The interesting design
calls were small and crisp:

1. **Two separate id namespaces.** `PanelId` for leaves, `BranchId`
   for dividers. A mouse drag holds a `BranchId` from press to
   release without worrying about pruning operations renumbering
   panels. Sharing one namespace would have conflated two
   semantically different handles.

2. **Atomic-commit grouping by "consumer makes scaffolding
   load-bearing".** `split` and `resize` shipped in the same commit
   because `resize` is the consumer that makes `Branch.{split,
   ratio, bounds}` load-bearing; splitting them across commits would
   have required `#[allow(dead_code)]` which violates the project's
   no-scaffolding-without-a-consumer rule. Lesson: commit boundaries
   aren't "one function per commit" — they're "one coherent
   capability per commit".

3. **Top-anchored resize again.** Same reflex as `Grid::resize`:
   walk the tree, redivide each branch by its stored ratio. No
   content scrolls in response to a window resize.

4. **Recursive consume helpers for tree mutation.**
   `close_node(Node) -> Option<Node>` takes ownership, walks
   recursively, returns the new subtree shape. The destructure-and-
   match-on-children pattern makes the four cases (both kept,
   promote left, promote right, both gone) explicit instead of
   spread across mutable references.

5. **Demo lives in the renderer crate, not the data crate.** The
   visual `layout_demo` needs `term_gpu`; putting it in
   `term_layout/examples/` would force a dep cycle. We kept the
   pattern from `render_term`: data crate stays dep-free, demo
   downstream pulls it in as a dev-dep. Three crates, six
   compilation units, zero cycles.

## Act 12 — `term_grid`: real shells in panels

The combined demo. Each leaf of the `PanelTree` owns a real
`portable-pty` shell. Reader thread per panel, `EventLoopProxy`
signalling, `encode_key` translating winit's logical-key events into
ANSI byte sequences. `Cmd+D` / `Cmd+Shift+D` / `Cmd+W` mutate the
tree and spawn / kill PTYs accordingly. Mouse drag resizes both the
panel and (eventually) the shell.

The "eventually" is the interesting bit:

1. **First version** called `sync_panels_to_tree` on every
   `CursorMoved`. zsh's "re-render the prompt on SIGWINCH" combined
   with our destructive `Grid::resize` (`row.resize(new_cols)` drops
   tail cells) gave a left panel filled with partial prompts
   stacked from drag history. Lesson: continuous gestures need a
   debounce; the destructive side-effect should fire on release,
   not on motion.

2. **Even with debounce**, the PanelTree's bounds shrink immediately
   on drag but the emulator stays at its pre-drag dimensions until
   release. Without render-side culling, glyphs from the larger
   grid spilled into the neighbouring panel. Lesson: when visual
   bounds can lag behind logical bounds, cull at the render step —
   the lag is a normal transient state, not a bug.

3. **Reflow is not free.** Tmux and alacritty wrap long lines on
   column shrink with a continuation marker, then unwrap on grow.
   The destructive first version shipped with the limitation
   documented; reflow landed in Act 13 below.

## Act 13 — Reflow: making resize non-destructive

Three commits, ~250 LoC including tests. The first two are
trivial — add a `WRAPLINE` bit to `CellFlags`, set it in
`Grid::print` on the auto-wrap branch. The third is the actual
reflow.

The algorithm came from Warp, not alacritty. Both have it; Warp's
`crates/warp_terminal/src/model/grid/flat_storage/index.rs::Index::rebuild`
is the cleanest mental model: walk old content in order, re-emit
at the new column count. Our cell-based grid simplifies this — no
flat byte buffer, no grapheme-run indexing, no `ByteOffset` math.
Just `chunks(new_cols)` over flat `Vec<Cell>` logical lines.

Three lessons stuck:

1. **Use a cell-level flag, not a per-Row field.** The flag lives
   on the row's trailing cell, not as a separate `Row.wrapped:
   bool`. Why does this matter? The flag survives cell mutation
   because the cell carrying it is at column `cols-1`, not the one
   being overwritten. Warp does this. Alacritty does this. We
   started with `Row.wrapped` and switched after reading
   `FlatStorage::add_row`.

2. **Cursor tracked by absolute row across multi-step resize.**
   Mid-resize `visible_start` shifts (because `rows.len()` mutates
   in both the reflow step AND the pad/truncate step). A
   visible-relative cursor mid-flow lands on the wrong row. The
   fix is a one-line discipline: keep a local `cursor_abs`,
   project to visible-relative at the very end.

3. **Drop trailing all-blank logical lines before re-wrap.**
   First test pass had `helloworld` ending up in scrollback. The
   empty rows below the cursor in the source buffer were becoming
   real rows in the rewrapped output, pushing visible_start down
   past the content. The outer pad-with-blanks step recreates
   trailing blanks already — re-emitting them is double-counting.

`term_grid` picks up reflow for free — `Grid::resize` signature
unchanged. Drag-divider release no longer leaves history
fragments. 12 integration tests in `tests/reflow.rs` pin the
behavior.

## Act 14 — SGR visual flags

The emulator had been parsing `CellFlags::{BOLD, ITALIC, UNDERLINE,
DOUBLE_UNDERLINE, STRIKE, FAINT, HIDDEN}` from day one of
`term_core`. The renderer ignored all of them — every cell shaped
with `Weight::NORMAL`, no decoration lines anywhere. Four atomic
commits closed the gap.

Two architectural calls worth flagging:

1. **Bold and italic come from system font faces, not synthesis.**
   `Attrs::weight(Weight::BOLD)` and `style(Style::Italic)` route
   through cosmic-text's fontdb to actual SF Pro Bold / Italic on
   macOS. No glyph stroking, no shader skew. The atlas caches them
   as distinct images because `cosmic-text::CacheKey` already
   includes weight/style — free deduplication.

2. **Decorations are rects, not glyph variants.** Underline,
   double-underline, and strike are `RectInstance`s in the rect
   pass at fixed vertical fractions of cell height (0.78 / 0.72-0.84
   / 0.42). Color = effective fg (already attenuated by FAINT if
   set). This keeps the atlas small (one glyph image per
   weight/style/codepoint, not per decoration combo) and the lines
   crisp at any DPI.

Faint = alpha × 0.5. Hidden = skip glyph push, keep bg and
decorations (matches xterm/iTerm). Blank cells with any decoration
bypass the "nothing to render" short-circuit so an underlined
space still draws its underline.

Sin: `term_grid` and `render_term` ended up with copy-pasted SGR
logic. Two consumers = below the DRY threshold; extraction waits
for a third (YAGNI).

## Act 15 — Scrollback with momentum, follow mode, jumps

The next user-visible Phase 6 item. Without scrollback, the demo
only shows the last visible_rows lines — barely usable as a real
terminal. The momentum integrator already existed in `scroll_demo`
(Phase 3.5 prototype); this work was port + multi-panel
integration + follow mode + finding a convention bug.

Architectural calls:

1. **Per-snapshot rendering, not a per-panel scroll uniform.**
   `RenderSnapshot.rows` grew to include the full buffer (scrollback
   + visible), and a `visible_rows: usize` field plus
   `visible_iter()` helper let existing consumers keep their old
   "just the visible region" view. `populate_panel` translates
   row indices to physical Y per cell, including the scroll
   offset. No new uniform, no new render pass — multi-panel just
   works.

2. **One in-flight gesture, keyed by panel id.** `PanelState`
   carries its own `ScrollState`, but `App.scrolling_panel`,
   `momentum_abort`, and `gesture_end_abort` are app-level and
   refer to whichever panel last got a wheel event. Per-panel
   momentum threads would be over-engineering for a UX where the
   user scrolls one thing at a time. `CustomEvent::MomentumTick(PanelId)`
   carries the panel id so stale ticks (after focus change or
   panel close) are dropped cleanly.

3. **Follow mode = capture state pre-change, act post.**
   `drain_panel` snapshots `was_at_bottom` from the current
   `ScrollState` BEFORE processing bytes. If true, after the
   buffer grows, it re-pins `offset_y` to the new bottom. Users
   who explicitly scrolled up stay where they were.

The convention bug deserves its own act:

## Act 16 — When a library doc tells you the opposite

`term_gpu::ScrollState` is documented with `offset_y == 0` at the
top of content and `max_offset` at the bottom. `term_grid` flips
this: 0 is at the bottom (cursor visible), max is at the top of
scrollback. The flip is deliberate — keeps the default state
"at the cursor" matching `ScrollState::default()`, and natural
macOS scrolling delivers positive wheel deltas on the
fingers-down gesture, which then increases `offset_y` toward
scrollback with no manual sign inversion.

Mid-implementation I "fixed" `populate_panel` to match
ScrollState's docs (commit `e56a33e`). The user reported scroll
felt inverted. The actual bug was in `drain_panel`'s
`was_at_bottom` check (it was comparing against `max_offset` —
the wrong side under our flipped convention). Once `was_at_bottom`
was fixed to `offset_y <= eps`, the original `populate_panel`
worked.

Lesson: library docs describe what the library was designed
around. When a downstream user inverts the convention deliberately,
mid-debug "let's align to the docs" can be the wrong direction.
The fix is to make every part of the downstream code consistent
with itself, not with the docs. The convention is now recorded as
a comment block in `populate_panel`.

## Act 17 — Selection

The next user-visible Phase 6 item after scrollback. Drag to
select cells, double-click for word, triple-click for row, Esc
clears. Copy doesn't ship in this act — clipboard is the next
work.

Three architectural calls worth flagging:

1. **Absolute row indices in `CellPoint`.** Selection coords
   point into `RenderSnapshot::rows` directly (scrollback +
   visible). User scrolls → highlight stays on its content; the
   renderer's baseline + scroll-offset math handles the
   projection. Visible-relative coords would shift the highlight
   on every scroll event.

2. **Clear on text change, keep on viewport change.** Warp's
   `app/src/terminal/model/selection.rs:1-6` doc-comment is
   explicit: "cleared when text is added/removed/scrolled". Our
   `drain_panel` clears after applying PTY bytes — except when
   the user is mid-drag in that panel (we'd kill an in-progress
   gesture). `sync_panels_to_tree` clears on column / row resize
   because reflow shuffles rows. User scroll leaves the
   selection alone.

3. **Mouse-mode gate.** Selection only starts when
   `emulator.mouse_mode() == MouseMode::None`. When Vim / htop /
   fzf set ButtonEvent or SGR mouse modes, their drag goes
   through the PTY instead. Without this gate we'd shadow in-app
   gestures.

Word / line selection (commit 72) is a straight port of Warp's
`DEFAULT_WORD_BOUNDARY_CHARS` list — same 33 punctuation chars
plus whitespace. Double-click walks left and right from the
clicked cell while the boundary-class matches.

## Act 18 — Clipboard (its own crate, image paste, layout-agnostic shortcuts)

After selection landed, the obvious gap was Cmd+C / Cmd+V. Took
a detour into Warp's clipboard module structure first — Warp
exposes a `Clipboard` trait and a rich `ClipboardContent` that
carries plain text, file paths, HTML, and images. We mirrored
the whole shape rather than the MVP "plain text only" version.
"Functional identity with Warp" is the bar the user set.

Three architectural calls:

1. **New crate, not a module under term_gpu.** Clipboard is
   platform integration, not rendering — and the precedent in
   Warp is `warpui_core::clipboard`, not buried inside
   `warpui`. Adding `term_clipboard` as a fourth sibling crate
   keeps responsibilities clean.

2. **Custom NSPasteboard FFI, no `arboard`.** Project pattern
   is "write our own", and `objc2-app-kit` is already in the
   tree via winit. Adding it explicitly costs a Cargo.toml line.
   The whole macOS backend is ~170 LoC including HTML, images,
   and file-path reading via NSURL.

3. **Image paste = save-to-temp + paste-path.** A terminal
   can't accept raw image bytes. The terminal side that makes
   Claude Code image input work: copy a screenshot, Cmd+V in
   CC's chat input, CC reads the temp file. The order of
   payload assembly (text → image filepaths → image data
   path) mirrors Warp's `process_paste_event` step-for-step.

A late-stage UX bug: Cmd+C on a Russian keyboard layout broke
because the `Key::Character` match expected "c", not "с". Fix
is universally applicable: match on `event.physical_key`
(KeyCode::KeyC) instead of `event.logical_key`. macOS apps have
always done this; we didn't realize until the user pointed it
out. Extended to every Cmd shortcut, not just C/V.

## Act 19 — Glyph cache fast-path: removing the per-cell `String` alloc

Performance was the first remaining item on the polish list, and
the user reminded me of it: "вспомнил, нужно проверить
производительность." Easy to ignore on a fast laptop with low
cell counts, harder to ignore once you run a real terminal at
200×60 cells × 60 fps — that's 720 000 cells touched per second
on the render path, and every one of them was allocating a
`String` for the shape cache key.

The audit was small: `TextShapeCache::shape(text: &str, ...)`
stored its entries keyed on `ShapeKey { text: String, ... }`,
and the key was built by `text.to_string()` at every call.
That's an alloc on cache *hit*, not just miss. At 60 fps the
allocator is doing more work than the rasterizer.

Warp had already solved this, and the user's brief was the
usual: "смотри на warp, как на эталон". A targeted read through
`warpui_core::fonts::Cache` and `app/src/terminal/grid_renderer/
cell_glyph_cache.rs` surfaced the structural answer:

- Two caches, not one. Single-codepoint cells use a `(char,
  FontId) → (GlyphId, FontId)` map. Combining-mark clusters use
  the bigger `(String, FontId) → …` map. The comment in Warp
  reads: *"avoid allocating strings when we don't need to!"*
- The fast path doesn't go through cosmic-text at all. It
  calls `font_face.glyph_index(char)` directly — a cmap lookup,
  no shape buffer, no BiDi analysis. That's exactly what we
  want for ASCII (the 99% case).

We followed the same shape:

1. `TextShapeCache::shape_char(font_system, ch, font_size,
   scale_factor, weight, style) -> Option<CharGlyph>`. Key is
   `(char, font_id)`, fully `Copy`, no allocation. On miss,
   resolve primary face via `FontSystem::db().query()` and
   read `Font::rustybuzz().glyph_index(ch)`. (cosmic-text's
   `Font::rustybuzz()` derefs to `ttf_parser::Face`, so we get
   the cmap query for free.)
2. `CharGlyph { font_id, glyph_id, baseline_y_physical }`
   carries enough to build a `CacheKey` and place the glyph
   at the cell origin without a single line of shape code.
   `baseline_y_physical = (ascender / units_per_em) *
   font_size_physical`, cached per `(weight, style)` so we
   pay for it once.
3. `prepare_shape_for_panel` chooses fast vs slow path per
   cell: `cell.extra.zerowidth.is_empty()` → fast path,
   otherwise fall through. Bold/italic still works because
   the face resolver keys on `(weight, style)`.
4. `CacheKey::new(font_id, glyph_id, font_size_physical,
   (cell_origin_x, baseline_y), CacheKeyFlags::empty())`
   returns the same atlas key cosmic-text would have produced
   via `LayoutGlyph::physical` — SubpixelBin binning preserved,
   so glyphs rasterized by the slow path are reused by the
   fast path and vice versa.

The whole change is a tight diff: ~160 lines in `text.rs` for
the API, ~80 lines at the callsite for the dispatch. Tests
were green on both sides (~250 in the workspace), but the real
verification is qualitative: type fast in a real shell with the
demo, watch nothing change visually. The expensive thing — the
720k/sec allocator pressure — is removed, and the rendering
pipeline is now structurally a peer to Warp's hot path.

Two atomic commits per the project's commit hygiene: API first
(`3aa2a33`), then integration (`e67b7c2`). The split mattered:
the API is independently useful for any future per-cell renderer
(e.g. an `anyclaude` panel) without committing to a particular
callsite.

## Act 20 — Phase 5: bootstrapping `anyclaude --gpu`

The point where everything we'd built had to actually run Claude
Code. Phase 5 turned out to be three orthogonal problems wearing
the same trench coat.

**The first half — uneventful integration** (~15 commits, C1-C9).
A `--gpu` flag in `main.rs` routed to a fresh `src/ui/gpu/` module
while the legacy ratatui path stayed alive next door. Skeleton →
shell PTY rendering → keyboard / scroll / selection / clipboard
parity with `term_grid` → top header + bottom footer chrome →
drop-shadow shader → three popup overlays (backend switch /
history / settings). Each commit was a tight diff because the
heavy lifting (cell rendering, scroll math, glyph cache) had
shipped in earlier phases. The `--gpu` flag let us verify
incrementally without breaking the legacy entry.

**Then C10a — full bootstrap**. Port the ~250 LoC of legacy
`runtime.rs` setup (Config, DebugLogger, tokio runtime,
ProxyServer + try_bind, TeammateShim, subagent hooks, proxy as
tokio task) into `gpu::run`. `ChildPty` started spawning claude
with the real env. Backend popup pre-selected the active backend;
Enter called real `switch_backend`. History pulled real switch
log. The GPU UI was running anyclaude for the first time.

And the rendering looked terrible.

**The second half — four rounds of Warp parity research.** The
first user screenshot showed Claude Code's welcome screen with
underlines under every line, a stretched alpaca logo, and double-
rendered title text. Three iterations of explore agents on Warp's
source produced FIX-1 (cell metrics from real ttf-parser
ascent/descent/line_gap, not `font_size * 1.2`), FIX-2 (native
painter for U+2580-259F block characters — solid rects, not
shaped glyphs), FIX-3 (non-sRGB swap chain + luma-dependent glyph
contrast curve from Windows Terminal). Block art tiled. Colors
became saturated. Lines underneath text stayed.

Three more rounds of static analysis on the SGR parser found
real adjacent bugs — `:` sub-param separator was unhandled, the
SGR-4 dispatcher mis-consumed sub-args as top-level params, alt-
screen entry/exit leaked SGR state. Each fix was correct.
Underlines persisted.

**The fourth round was different.** We dispatched an agent that
ran Claude Code under `script -q -F` and captured the actual
bytes. The trace said: claude never emits a single `CSI 4 m`
during the welcome screen. The previous three fixes were chasing
sequences that didn't exist. The real culprit, on line 6 of the
trace, was `CSI > 4 ; 2 m` — XTERM's `modifyOtherKeys = 2`
extension. Our `dispatch_csi` only treated `?` as a private
marker; for `>`, `<`, `=` it fell through to plain SGR. Claude's
extended-keyboard handshake at startup was being dispatched as
SGR 4;2 → permanent DOUBLE_UNDERLINE on every cell.

One line of code reject those sequences; the underline went away.

**The lesson, painfully**: when a terminal-rendering bug looks
like wrong attributes, the FIRST step is to capture the PTY
bytes. Three rounds of static analysis missed what one PTY trace
made obvious. We saved this as a workflow memory
(`feedback_capture_pty_bytes_for_render_bugs`) so it doesn't get
re-learned.

**MVI mid-story**: user feedback midway through the second half —
"ВЕСЬ ui должен быть на mvi". I'd refactored the popup state to
inline `Option<Popup>` enum + field-mutating handlers because it
was the simplest thing that worked. The user reminded that the
project had an mvi crate (Store + Actor primitives) that was
mandatory for ALL UI design. One refactor commit later, the
popups dispatched intents to `Store<BackendSwitchActor>`,
`Store<HistoryActor>`, `Store<SettingsActor>` — using the existing
actor implementations from the legacy ratatui path. The lesson:
when a codebase has an architecture convention, follow it even
when "simpler" looks possible — the convention is usually load-
bearing for things you don't see.

Phase 5 isn't fully done — a handful of visible bugs remain
(title double-render, cursor placement, popup section split,
header sub/team labels). Tracked in
`gpu-terminal-remaining-bugs.md`. The cutover commit (delete
ratatui code, remove the `--gpu` flag) is deferred until those
settle.

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
10. **A 770-LoC hand-rolled VT parser beats a dependency** when the
    crate is meant to be self-contained. The Paul Williams diagram
    is well-trodden ground; following it carefully is less work than
    living with someone else's API decisions.
11. **Logical and visual models can disagree.** Our cell grid is
    monospace (VT semantics demand it). Our renderer shapes
    variable-width fonts. Separating "what the cursor addresses"
    from "what the user sees" is the unlock.
12. **DA replies are not optional.** Apps that send `CSI c` block
    waiting for an answer. Always answer.
13. **Read the research, then build.** Each phase we did
    delegated-agent research against Warp before writing code. Both
    times we found something the spec missed (subpixel via
    cosmic-text, Cell vs TextRun grid). Cheap to ask, expensive to
    rewrite.
14. **Ignore shaper advances for cell positioning.** Even when you
    use a shaper (you need to, for ligatures and combiners),
    don't let it dictate where glyphs land. Place each glyph at
    `col × cell_width` (integer physical px); the shaper's job is
    to pick the right glyph image, not its X coordinate. This is
    Warp's hot path and the reason their text aligns cleanly across
    rows; ours blurred until we adopted it.
15. **YAGNI doesn't extend to fields with non-obvious downstream
    consumers.** Removing `self.scale_factor = renderer.scale_factor()`
    seemed safe in commit 1 (nothing read it). Commit 2 added a
    consumer; the field was already gone; the regression cost two
    rounds of misdirected debugging. When a field looks unused,
    grep for its writers first — if it's the bridge between two
    subsystems, leave it.
16. **Loop bounds derived from mutable state will hang.** Our
    `Grid::resize` looped `while self.rows.len() <
    self.visible_start() + rows` — and `visible_start()` derived
    from `rows.len()` too. Each push extended the bound. Snapshot
    your loop targets into `let` bindings before the loop body
    starts.
17. **Top-anchored vs alacritty-style resize is a UX call, not a
    technical one.** Both work; both have tests. Pick the one your
    users actually want — for us it was "контент не двигается".
18. **Atomic commit grouping is by capability, not by function.**
    Splitting `split` and `resize` across commits would have meant
    `#[allow(dead_code)]` on `Branch.{split, ratio, bounds}`;
    grouping them gave one cohesive "tree becomes mutable" commit.
    "One logical change per commit" wins over "one function per
    commit".
19. **The data crate stays dep-free; demos live downstream.** Both
    `term_core` and `term_layout` have zero deps and visual demos
    in `term_gpu/examples/`. Three crates, zero cycles, four
    end-to-end demos — `scroll_demo`, `render_term`, `layout_demo`,
    `term_grid`.
20. **Continuous-gesture mutations need a debounce, especially with
    side effects.** A divider drag fires `CursorMoved` dozens of
    times per second. If each event calls `pty.resize` and
    `emulator.resize`, the shell sees SIGWINCH spam, re-renders the
    prompt every frame, and (with our destructive `Grid::resize`)
    you get history fragments stacked all the way down the panel.
    Keep the gesture's visual update on motion; defer the
    destructive side effect to release.
21. **When two layers can disagree, cull at the renderer.** The
    PanelTree's bounds and the emulator's `(cols, rows)` are
    deliberately out of sync during a drag — tree updates on motion,
    emulator updates on release. The renderer culls cells past the
    panel's bounds so the lag is invisible. Architecturally this is
    just "the renderer trusts no one"; pragmatically it removes a
    whole class of "data hasn't propagated yet" rendering glitches.
22. **`Cmd` is for app shortcuts; `Ctrl` is for the shell.** Demo
    shortcuts (split / close / quit) sit on `Cmd`; every `Ctrl`
    combo flows through `encode_key` to the PTY. No conflicts
    with the shell's signal handling (Ctrl+C, Ctrl+Z) or readline
    (Ctrl+A, Ctrl+E, Ctrl+R, …).
23. **Cell-level wrap marker is better than a per-Row field.**
    Warp and alacritty put soft-wrap state on `row[cols-1].flags`
    (bit 12 = `WRAPLINE`), not on `Row` itself. The flag survives
    cell mutation because it lives on a different index than the
    one being overwritten, and `Row` stays a pure cell container.
    Free architectural simplification, picked up from
    `FlatStorage::add_row`'s wrap detection.
24. **"Top-anchored grow" absorbs scrollback into the viewport.**
    When `visible_rows` grows, the new vertical space pulls
    scrollback content back into view. The formula is
    `scrollback_to_keep = prev_scrollback - visible_increment`,
    not `target = prev_scrollback + new_visible`. The latter pins
    scrollback length and starves the new vertical space — content
    you expected to see after the window grew stays hidden.
25. **Trim trailing all-blank logical lines before re-wrap.** The
    outer pad-with-blanks step recreates them; re-emitting them
    inside reflow double-counts and pushes real content into
    scrollback. Discovered when a `"helloworld"` round-trip
    through shrink+grow disappeared into the scrollback that
    didn't exist before the resize.
26. **Bold/italic via font faces, not synthesis.** Cosmic-text's
    `Attrs::weight/style` route through fontdb to actual system
    faces; `CacheKey` includes them so the atlas caches bold and
    regular as distinct glyphs naturally. Stroking glyphs or
    skewing in the shader to fake the look would have been
    weeks of work for a worse result.
27. **Text decorations belong in the rect pass.** Underline,
    strike, double-underline are thin `RectInstance`s at fixed
    fractions of cell height — not baked into the glyph image. One
    glyph in the atlas per weight/style/codepoint, regardless of
    decoration combos. Crisp lines at any DPI as a bonus.
28. **Below-DRY-threshold duplication is fine.** `term_grid` and
    `render_term` carry identical SGR plumbing. Extracting it
    when there are only two consumers buys nothing; YAGNI says
    wait for the third.
29. **One in-flight gesture, keyed by id, beats per-thing threads.**
    Per-panel momentum threads would be gratuitous for a UX where
    users scroll one thing at a time. A single
    `App.scrolling_panel: Option<PanelId>` and a single momentum
    abort handle do the job; the `CustomEvent::MomentumTick(PanelId)`
    payload makes stale ticks easy to ignore when focus moves.
30. **Capture user state pre-change, act on it post.** Follow mode
    snapshots `was_at_bottom` BEFORE applying bytes; checks AFTER
    would see the new `max_offset` and lose the signal. Generalizes
    to any "auto-anything when something shifts" decision.
31. **Library doc conventions are not laws for downstream callers.**
    `ScrollState`'s docs say `offset_y == 0` is the top. `term_grid`
    flips this so 0 is at the cursor (matches `Default::default()`
    and natural macOS scroll direction with no sign inversion).
    Mid-debug I "aligned to the docs" and broke the wheel feel.
    The fix wasn't to change the convention — it was to make
    every comparison in `term_grid` consistent with its own
    chosen convention.
32. **Absolute coords let selections survive viewport changes.**
    Selection points reference rows by their index in
    `RenderSnapshot::rows`, not by their visible position.
    Scroll moves the viewport; row indices don't move; the
    highlight stays on its content. The same reasoning applies
    to any cell-pointing feature (search highlight, link
    target) — store by absolute index, project to viewport at
    render time.
33. **Carve out the in-progress gesture from auto-clears.**
    Warp's selection rule is "clear on text change". Strict
    application would kill an in-progress drag the moment the
    shell prints. The right fix is a one-line carveout:
    `if dragging_selection != Some(id)` skip the clear. Same
    pattern applies to any "auto-something" rule that runs while
    a user gesture is live.
34. **Custom FFI > cross-platform crate when project pattern says so.**
    `arboard` would have been a 3-line clipboard. We went with
    `objc2-app-kit` (already in our tree via winit) and ~170 LoC
    of NSPasteboard FFI. The pattern across the project — own VT
    parser, own grid, own BSP layout, no ratatui / alacritty —
    isn't dogma, it's consistency. New platform code should
    match.
35. **Image paste = save-to-temp + paste-path.** Terminal stdin
    is a byte stream. The bridge between clipboard image data
    and any image-aware CLI (Claude Code chat, vim+image
    plugins, etc.) is "write to /tmp, paste the path,
    shell-quoted". iTerm, Warp, and us all converge on this. No
    inline image protocol needed.
36. **Match Cmd shortcuts on physical_key, not logical_key.**
    `Key::Character("с")` (Russian) ≠ `"c"`. Hardware key
    position is what users physically remember and what macOS
    apps universally bind to. The fix is one line per shortcut —
    `PhysicalKey::Code(KeyCode::KeyC)`. Easy to miss until a
    user reports it.
37. **Single-threaded by construction beats serial_test.**
    NSPasteboard from multiple non-main threads SIGSEGVs.
    Instead of pulling a `serial_test`-style crate, fold all
    scenarios into one `#[test]` function. Trades a single
    "many asserts" function for zero extra deps; fine when the
    surface is small.

## Possible follow-up articles

- "From 60 to 240 FPS: profiling a wgpu terminal"
- "Why we chose `Vec<Row>` over `sum_tree` for a terminal grid"
- "BSP panels for terminal UI: lessons from tmux"
- "Integration day: wiring term_core into anyclaude"
- "Top-anchored vs alacritty: two terminal resize semantics"
