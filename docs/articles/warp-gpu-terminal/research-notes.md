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

- 3 custom crates: `term_core` (VT parser, not yet built), `term_gpu`
  (renderer + atlas + scroll + text), `term_layout` (BSP panels, not
  yet built).
- 6 external deps: `wgpu`, `winit`, `cosmic-text`, `futures`,
  `futures-timer`, `glam`.
- 2 render pipelines: `rect` and `text`. (Image is optional, planned
  for later.)
- `Vec<Row>` of `TextRun` for the cell grid. No `sum_tree`.
- Pixel-based scroll with Warp's 7-constant momentum integrator.
- `RGBA8Unorm` glyph atlas with frame-counter eviction.
- cosmic-text shape cache with the same frame-counter eviction.

## 10. Lessons learned during text rendering

### DPI awareness: one field, one multiplication

The naive way of doing high-DPI is to scatter `* scale_factor` calls
through every layout site. That's a recipe for bugs (we shipped one —
text was half size on Retina).

The clean way: author every instance position and size in **logical
pixels**, add `scale_factor` to the `Uniforms` struct, and multiply
once in the vertex shader before the NDC transform:

```wgsl
let px_logical = in.pos + q * in.size - uniforms.scroll_offset;
let px_physical = px_logical * uniforms.scale_factor;
let ndc = (px_physical / uniforms.screen_size) * 2.0 - 1.0;
```

cosmic-text gets `font_size * scale_factor` so it rasterizes at
physical density; we divide returned glyph positions by `scale_factor`
to get logical coordinates back. One conversion at the rasterization
boundary, then everything is logical.

### cosmic-text subpixel is automatic

We planned to port Warp's `SubpixelAlignment` (3 X-bins, snap Y in
shader). Then we read the cosmic-text source: `CacheKey` already
includes `x_bin` and `y_bin` of type `SubpixelBin` (4 variants each).
Caching by the full `CacheKey` gets us subpixel-correct glyph images
for free. Memory cost ×16 per glyph variant vs Warp's ×3 — we accept
the extra memory for zero hand-rolled code.

Lesson: read library docs before reimplementing the cool thing.

### Shape cache mirrors atlas eviction

Like the atlas, the shape cache uses a frame-counter (`last_used_frame
+ MAX_UNUSED_FRAMES`) instead of an LRU. Pattern is identical: simpler
than intrusive linked lists and adequate for the workload.

Two thresholds: atlas at 10 frames (~0.16 s) because glyphs come and
go fast during scroll; shape cache at 60 frames (~1 s) because shaped
text tends to be more stable. Both tuned empirically; either could
move.

### CPU culling is a one-liner

The cull predicate is two comparisons:

```rust
fn in_view(origin_y: f32, height: f32, scroll_top: f32, viewport_h: f32) -> bool {
    origin_y + height > scroll_top && origin_y < scroll_top + viewport_h
}
```

Per-frame impact on the demo (1000 stripes × 1 row label per 10
stripes = 100 labels): with culling on a 720 px viewport, ~10
labels actually shape and atlas-lookup; 90 get skipped.

### WGSL `vec3` alignment is 16, not 12

We tried to add a `vec3<f32>` pad at the end of a uniform struct.
Both shaders compiled, validation failed at first draw:

> Buffer is bound with size 32 where the shader expects 48

`vec3<f32>` in WGSL has alignment 16 (not 12 — the WGSL spec is
explicit about this, matching std140 historical rules). The struct's
final size rounds up to a multiple of the largest member's alignment,
so the vec3 pushed total size from 32 to 48 bytes.

Fix: three scalar `f32` pads instead. Each has align 4, so the struct
stays 32 bytes and matches the Rust `repr(C)` layout.

Generalisable: if writing uniforms by hand (we don't use `bytemuck`
intentionally — see spec §5.3), avoid `vec3` and `mat3` in struct
fields. Use `vec4`/`mat4` or scalar pads.

## 11. Phase 1: VT parser as a 0-dep crate

After rendering was solid, we ran a second research pass against
Warp focused on VT parsing. Two material findings shifted the plan
before we wrote a line of `term_core`.

### Warp uses (a fork of) `vte`, but we don't

Warp wraps the `vte` crate (originally an Alacritty subproject) and
implements `vte::Perform`. We could have done the same; we chose to
hand-roll the Paul Williams state machine in ~770 lines of std-only
Rust. Reasons:

- `term_core` is the dependency root of the whole terminal pipeline.
  Keeping it self-contained makes blame trivial.
- The state diagram is exhaustively documented
  (<https://vt100.net/emu/dec_ansi_parser>). Following it is less
  work than living with someone else's `Perform` API.
- Our `Action` enum exposes exactly the variants we handle, no more,
  no less. No "this method exists but does nothing".

The "don't defer features Warp ships" rule didn't apply here — `vte`
is an implementation choice, not a feature. We ship the same
*capabilities* (P0+P1 sequences) without the dep.

### Fixed-cell grid, alacritty-style — not our original TextRun plan

Our first spec had a variable-width grid: `Row { runs: Vec<TextRun> }`
with `TextRun { text: String, fg, bg, flags }`. Beautiful for a
text editor; broken for a terminal:

- `CUP row 5 col 10` must address a definite cell. With variable-
  width spans, "col 10" is ambiguous — is it the 10th char of `text`
  concatenated, or the 10th visual column?
- ECH/DCH/ICH (the P0 edit primitives ink uses constantly) operate
  on cell ranges. Translating to span-rewrites is awkward and slow.

Warp uses `Row { cells: Vec<Cell> }` with `Cell { c: char, fg, bg,
flags, extra: Option<Box<CellExtra>> }`. We took the same model.
Variable-width *rendering* happens in `term_gpu` — at shape time, we
ask cosmic-text for per-cell advances and lay glyphs accordingly.
Logical model is monospace; visual model is variable. Best of both.

The `Box<CellExtra>` indirection is a classic optimisation: the
common-case cell stays small (~24 bytes), the rare cases
(combining marks, OSC 8 hyperlinks, OSC 133 prompt markers) live on
the heap.

### Sequences our first spec missed

Research §3 identified ~30 P0/P1 sequences our spec didn't list.
The critical ones:

- **`CSI X` ECH** — erase chars at cursor without moving it. ink
  uses this on every redraw to clear partial lines.
- **`CSI P` DCH / `CSI @` ICH** — delete/insert chars; ink uses these
  for in-place editing.
- **`CSI b` REP** — repeat last char N times. Box-drawing apps use
  this a lot.
- **`CSI c` DA** — device attributes. Apps **send this and block
  waiting**. Without a reply (we answer `\x1b[?6c`), some apps hang
  at startup.
- **`CSI d` VPA + `CSI E`/`F` CNL/CPL** — vertical positioning.
- **DEC 6 DECOM** — origin mode. Without it, `CUP` inside a
  scrolling region is wrong.
- **DEC 7 DECAWM** — autowrap. Almost universally on.
- **DEC 1004 focus reporting** — apps subscribe and expect
  `CSI I` / `CSI O` on focus changes. ink uses this to dim
  background panels.
- **DECSET 2026 synchronized output** — modern, batches output
  frames to eliminate flicker.

We also missed three OSCs worth implementing:

- **OSC 7** — shell announces its CWD as a `file://` URI.
- **OSC 8** — hyperlinks. Warp doesn't actually handle these (their
  `osc_dispatch` falls through), but modern apps emit them and our
  attaching them to `Cell::extra.hyperlink` is cheap.
- **OSC 133** — FinalTerm prompt markers (A/B/P). Lets the renderer
  identify prompt regions for future block-style UI.

### OSC stickiness model

Two different attachment semantics emerged:

- **OSC 8 hyperlinks are sticky** — once set, they apply to every
  subsequent printed cell until a closing OSC 8 (empty URL) appears.
  Implemented as `Grid.current_hyperlink: Option<(String, String)>`.
- **OSC 133 prompt markers are one-shot** — they tag the next
  printed cell only. Implemented as `Grid.next_prompt: Option<…>`
  that `Grid.print` takes (clears) on first attach.

`Grid::print` checks both and lazily allocates `Cell.extra` only if
either is active. Common path stays a flat copy.

### Tests in `tests/`, not `src/`

Project policy is "no `#[cfg(test)] mod tests` in `src/`; integration
tests in `crates/<crate>/tests/`". We violated this in commit 4
(parser) and caught it before the commit landed. Two reasons it
matters:

- `dead_code = "deny"` workspace lint can fire on test-only helpers.
- Integration tests prove the public API works as advertised; unit
  tests inside `src/` can rely on private state and silently break.

The fix is mechanical: move the `mod tests` block to a sibling file
in `tests/`. We ended up with 39 tests across `parser_smoke.rs`
(20) and `emulator_smoke.rs` (19), all hitting public API only.

### What we still don't ship (and won't)

Per spec §4.3 + research §4:

- Tmux control mode (Warp-specific UX).
- Image protocols (Kitty APC, iTerm OSC 1337, sixel). Claude Code
  doesn't emit images.
- OSC 4 / 10 / 11 / 12 palette manipulation. Claude Code doesn't
  change the palette.
- Warp's own OSC ID space (9277..9280, 781378).
- Kitty keyboard protocol — defer until observed in real traces.
- DEC charsets G2/G3. G0+G1 covers 99.9% of TUI apps.

## 12. The "don't defer features Warp ships" rule

The most important process lesson came from a user pushback. When we
demoed text rendering, three optimisations weren't in: shape caching,
CPU culling, font fallback config. The plan was "Phase 3 = baseline,
Phase 5 = polish".

User response:

> "Why aren't these implemented? Warp is right there as a reference."

He was right. Phase 5 was supposed to be integration, not catch-up.
Deferring optimisations the reference implementation already does
creates technical debt that compounds across phases, and ships a
weaker product at every milestone.

The rule, now encoded as a memory: **anything Warp does in the area
you're working on belongs in the current phase**. Not "I'll add later",
not "for the prototype". The plan is to match Warp's quality, full
stop. If a feature legitimately doesn't apply, say so explicitly with
a reason (e.g. we skip `sum_tree` because a VT cell grid isn't a code
editor) — but don't pretend it's phasing.

Good rule to print on a wall: **build the real thing each phase.**

Everything else (selection, scrollback navigation, drop-shadow on
overlays) is genuine polish — features Warp also defers until later.

## 13. Mini-integration — surprises when crates meet

After `term_core` and `term_gpu` worked in isolation we plumbed them
through a new example, expecting the visual quality from the scroll
prototype to carry over. It didn't, on first try. Four debugging
rounds, two `term_core` bugs, and a UX call later, three findings
that belong in the article:

### Per-cell snap isn't enough — ignore advances entirely

Per-run shaping put `Hello world` through cosmic-text once, snapped
the run origin to an integer pixel, and let glyphs ride the shaper's
natural advances. On Retina with `cell_width ≈ 8.4 px`, each letter
landed on a different `SubpixelBin` (Zero through Three), each
rasterised at a different fractional offset, the GPU sampler blended
neighbouring atlas pixels — and the whole row read as soft.

We tried per-cell snap on cell origin: better, not crisp. Y-snap on
baseline: marginally better. Real fix came from a second research
agent pass against Warp's `paint_line`:

```rust
// app/src/terminal/grid_renderer.rs:1491
fn paint_line(line: &Line, baseline: Vector2F, cell_width: f32, ...) {
    for run in &line.runs {
        for glyph in &run.glyphs {
            let glyph_x = character_index_to_cell_map[glyph.index] as f32
                          * cell_width;
            let glyph_origin = baseline + vec2f(glyph_x, 0.);
            scene.draw_glyph(glyph_origin, glyph.id, ...);
        }
    }
}
```

Warp **ignores `LayoutGlyph.x`**. Every glyph lands at
`col_index × cell_width`, where `cell_width = round(advance_M)` is
integer physical pixels (from `grid_size_util.rs`). Shaper output
is just for choosing the right glyph image — the shaper has no say
on positioning. Adopting this killed our remaining alignment drift.

### The blur was DPI, not subpixel

Three rounds of subpixel fixes made things marginally better but
never crisp. Real cause was a YAGNI regression in bootstrap commit:

```rust
// What I had in commit 1, with no consumer in that commit:
// self.scale_factor = renderer.scale_factor();

// What I removed it to. Field defaulted to 1.0. By commit 2 the
// shape calls used self.scale_factor, but the field hadn't been
// updated — glyphs rasterised at logical-pixel size, sampler
// bilinearly stretched them ×2 to Retina.
```

The framebuffer is in physical pixels, the shape calls have to
match. A single field, two-line restore, instantly crisp. The
lesson is in [[feedback-solid-dry-kiss-yagni]]: YAGNI doesn't
apply to fields whose downstream consumers exist in the *next*
commit — verify the lack of consumer before deletion.

### Top-anchored resize because the user said so

Standard terminal behaviour (alacritty/xterm/Warp default) on
shrink: scroll top rows into scrollback, cursor stays at bottom.
On grow: pull rows back. We implemented this initially. User push:

> "у меня варп настроен так, что контент внутри него ресайзится,
> но не двигается вверх, вниз или куда либо еще"

So their actual Warp setup pins content top, doesn't scroll on
resize. The default is configurable. We rewrote `Grid::resize`:

```rust
let target = self.scrollback_len() + rows;
if self.rows.len() < target {
    while self.rows.len() < target { self.rows.push(Row::new(cols)); }
} else if self.rows.len() > target {
    self.rows.truncate(target);  // drop bottom rows; lost forever
}
self.cols = cols;
self.visible_rows = rows;
self.scroll_bottom = rows.saturating_sub(1);
if self.cursor_row >= rows { self.cursor_row = rows.saturating_sub(1); }
```

Trade-off the user accepted: shrinking past existing content loses
that content (it's not pushed into scrollback). Test in
`tests/emulator_smoke.rs::resize_keeps_top_content_anchored_through_shrink_and_grow`
asserts the contract.

## 14. Phase 4 — BSP layout: the small data-structure crate

`term_layout` is ~250 LoC of recursive `Box<Node>` BSP, zero
external dependencies. Most of the work was deciding what NOT to
add:

- **No `slotmap` / arena.** Recursive `Box<Node>` ownership is
  simple and the tree never grows past a few dozen nodes.
- **No `parent` pointers.** Trees are walked from the root each
  operation; the constant factor is irrelevant at this size.
- **No persistent layout (a la React).** Mutation is in-place.
- **No external focus management.** `set_focus(id) -> bool` and
  done. Navigation by direction (Cmd-Alt-Arrow) is a Phase 5
  concern — not the data structure's job.

The atomic-commit grouping was the one design call worth talking
about. `split` and `resize` shipped in a single commit because the
fields they share (`Branch.{split, ratio, bounds}`) need both
operations to be load-bearing — splitting them would have meant
writing the fields with `split` and reading them with `resize`,
across two commits, with `#[allow(dead_code)]` in between. The
project's "no scaffolding without a consumer" rule
([[feedback-solid-dry-kiss-yagni]]) made the call: combine into one
"tree becomes mutable" commit.

Two id namespaces — `PanelId` for leaves, `BranchId` for dividers —
keep semantically different handles separate. A mouse drag holds a
`BranchId` from press to release without caring about panels being
renumbered; a content payload (term emulator) holds a `PanelId`
without caring about dividers shifting.

The end-of-Phase-4 demo (`crates/term_gpu/examples/layout_demo.rs`)
renders panels as coloured rects with a slim semi-transparent
focus border. Cmd+D / Cmd+Shift+D / Cmd+W keyboard shortcuts;
mouse click to focus; mouse drag to resize dividers. Visual smoke
test for the whole crate; runs at 120 fps on Retina.

## 15. term_grid — first real terminal

Combined demo. Each leaf in the `PanelTree` owns a real
`portable-pty` shell. Reader thread per panel, keyboard input
encoded to ANSI bytes and written to the focused PTY,
divider drag resizes both layout and shells. The findings:

### SIGWINCH spam needs a debounce

First version: `sync_panels_to_tree` ran on every `CursorMoved`.
During a drag winit fires the event dozens of times per second.
zsh re-renders its prompt on every SIGWINCH — combined with our
destructive column-shrink (`row.resize(new_cols)` drops cells past
`new_cols`), the left panel filled with partial prompts stacked
from drag history:

```
artem
artem
artem
@Arte
ms-Ma
cBook
artem
…
```

Fix: defer the sync to `on_mouse_release`. Tree mutates on motion
(visual immediate); shell receives one SIGWINCH at the end. This
applies to any continuous gesture with a destructive side effect —
keep the visual update on motion, defer the destructive part to
release.

### Tree bounds and emulator bounds are deliberately out of sync

After the debounce fix, a new problem: the PanelTree shrinks
immediately during a drag, but the emulator (still at its pre-drag
dimensions, awaiting the deferred sync) keeps rendering its full
grid. The (now-larger) glyph grid spilled into the neighbouring
panel. Fix: cull at the renderer.

```rust
let panel_max_x_phys = panel_rect.w * sf;
let panel_max_y_phys = panel_rect.h * sf;
for (row_idx, row) in snapshot.rows.iter().enumerate() {
    let row_y_phys = row_idx as f32 * metrics.height_physical;
    if row_y_phys >= panel_max_y_phys { break; }
    for (col_idx, cell) in row.cells.iter().enumerate() {
        let col_x_phys = col_idx as f32 * metrics.width_physical;
        if col_x_phys >= panel_max_x_phys { break; }
        // …emit cell glyph + bg rect…
    }
}
```

Same idea for `build_cursor_rect`: return `None` when the cursor's
cell origin falls outside `panel_rect`. Architecturally this is
just "the renderer trusts no one"; pragmatically it removes a whole
class of "data hasn't propagated yet" rendering glitches when two
layers update on different cadences.

### Cmd is for the app; Ctrl is for the shell

`encode_key` maps `Ctrl + letter` to the corresponding ASCII
control byte (Ctrl+C → 0x03, Ctrl+D → 0x04, …) and ships it to the
PTY. `Alt + key` gets ESC-prefixed for Meta. `Cmd` combos are
intercepted by the demo (Cmd+Q exits, Cmd+D splits, Cmd+W closes)
and never reach the shell — which means there's no conflict with
the shell's own use of `Ctrl`-combos for signal handling and
readline shortcuts.

### Reflow lands (Phase 6 partial)

The destructive column-shrink described above was fixed in three
atomic commits (`4e5c5e2`, `901ed78`, `e2a4c4b`). The algorithm
came from Warp, not alacritty — `Index::rebuild` in
`crates/warp_terminal/src/model/grid/flat_storage/index.rs` is the
clearest reference. Both projects mark soft-wrap with a per-cell
flag (`Flags::WRAPLINE` on `row[cols-1]`), not a per-row field.

Adapted for our cell-based grid (no flat byte buffer, no
grapheme-run indexing):

```rust
fn reflow_columns(&mut self, new_cols: usize) -> Option<usize> {
    let cursor_abs_row = self.visible_start()
        + self.cursor_row.min(self.visible_rows.saturating_sub(1));
    let (cur_line, cur_offset) = locate_cursor_logical(
        &self.rows, self.cols, cursor_abs_row, self.cursor_col,
    );

    let logical = collect_logical_lines(&self.rows, self.cols);
    let new_rows = rewrap(&logical, new_cols);
    let (new_abs_row, new_col) = place_cursor_logical(
        &logical, cur_line, cur_offset, new_cols,
    );

    self.rows = new_rows;
    self.cursor_col = new_col;
    Some(new_abs_row)
}
```

Three findings worth keeping:

1. **Cell-level flag, not per-Row field.** Started with
   `Row.wrapped: bool` (cleaner Rust API), switched after reading
   Warp's `FlatStorage::add_row` — it does
   `row[cols-1].flags().intersects(WRAPLINE)`. The flag lives on a
   different cell than the one being overwritten, so it survives
   cell mutation. `Row` stays a pure cell container.

2. **Drop trailing all-blank logical lines before re-wrap.** First
   test pass had `helloworld` ending up in scrollback after a
   shrink → grow round-trip. The empty rows below the cursor were
   becoming real rows in the rewrapped output, pushing visible_start
   down past the content. The outer pad-with-blanks step recreates
   trailing blanks already; re-emitting them inside reflow
   double-counts.

3. **"Top-anchored grow" absorbs scrollback.** Initially the outer
   `Grid::resize` computed `target = prev_scrollback + new_visible`,
   which pinned scrollback length and starved the new vertical
   space. The fix: `scrollback_to_keep = prev_scrollback -
   visible_increment` — when the window gets taller, old scrollback
   slides back into view. Matches the user's "content does not
   move on resize" mental model.

`term_grid` picks this up via the unchanged `Grid::resize`
signature — drag-divider release no longer leaves history
fragments. 12 integration tests in `tests/reflow.rs` pin the
behavior; the existing render-side cull (transient drag state)
stays as-is.

### `portable-pty` as a dev-dependency

Same pattern as `term_core` and `term_layout`: external systems
(processes, in this case) live in the demo's dev-dependencies, not
in `term_gpu`'s runtime dependencies. Three crates, zero cycles,
four end-to-end demos.
