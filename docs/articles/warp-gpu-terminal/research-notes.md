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

## 16. SGR visual flags (term_gpu, Phase 6 partial)

Emulator was emitting `CellFlags::{BOLD, ITALIC, UNDERLINE,
DOUBLE_UNDERLINE, STRIKE, FAINT, HIDDEN}` since Phase 1; the
renderer ignored them all. Four atomic commits closed the gap
(`79da3d7`, `3b704e9`, `835d680`, `675c92d`), ~200 LoC plus docs.

### Bold/italic via cosmic-text face switching

The minimum viable change is at the shape cache:

```rust
pub fn shape(
    &mut self,
    font_system: &mut FontSystem,
    text: &str,
    font_size: f32,
    scale_factor: f32,
    wrap_width: Option<f32>,
    weight: Weight,    // new
    style: Style,      // new
) -> &ShapedText { ... }
```

Internally:

```rust
let attrs = Attrs::new()
    .family(family.as_cosmic())
    .weight(weight)
    .style(style);
```

`Weight` and `Style` get re-exported from `term_gpu`, so the
demos `use term_gpu::{Weight, Style}` instead of pulling
cosmic-text into their dep tree. `populate_panel` derives them
from `cell.flags`:

```rust
let weight = if cell.flags.bold() { Weight::BOLD } else { Weight::NORMAL };
let style  = if cell.flags.italic() { Style::Italic } else { Style::Normal };
```

`cosmic_text::CacheKey` already contains `weight` and `style`, so
the atlas caches bold-`h` and regular-`h` as distinct glyph
images naturally — no extra cache-key plumbing required.

### Decorations are rects, not glyph variants

Underline / double-underline / strike land in the rect pass at
fixed cell-height fractions, color = effective fg:

```rust
if cell.flags.underline() {
    rects.push(RectInstance {
        pos: [pos_x_logical, pos_y_logical + cell_h_logical * 0.78],
        size: [cell_w_logical, 1.0],
        color,
    });
}
```

Vertical positions:

| Decoration | y fraction of cell height | thickness (logical px) |
|---|---|---|
| Underline | 0.78 | 1.0 |
| Double underline (upper) | 0.72 | 0.8 |
| Double underline (lower) | 0.84 | 0.8 |
| Strike | 0.42 | 1.0 |

These are calibrated for a 1.3 line-height ratio and SF Pro
metrics. If we adopt a more compact line-height in the future or
switch font families, the positions may need re-tuning; treat
them as constants that come with the font choice, not as
universals.

### FAINT and HIDDEN

```rust
if cell.flags.faint() {
    color[3] *= 0.5;
}
let push_glyph = !cell.flags.hidden() && !is_blank;
if push_glyph { /* shape + push */ }
// decoration rects still emit regardless of hidden
```

Faint is alpha attenuation only — no separate "faint color" curve.
Hidden suppresses the glyph push but keeps bg and decorations
(matches xterm/iTerm/Warp). Trying to make HIDDEN a full
"cell doesn't exist" toggle would break cursor positioning and
selection later, so we explicitly stop at "glyph suppressed".

### The blank-cell short-circuit needs a decoration check

Original code:

```rust
let is_blank = cell.c == ' ' || cell.c == '\0';
if is_blank && fg_eff == TermColor::Default {
    continue;
}
```

An underlined blank space would skip its underline. New version
gates on decoration flags too:

```rust
let has_decoration = cell.flags.underline()
    || cell.flags.double_underline()
    || cell.flags.strike();
if is_blank && fg_eff == TermColor::Default && !has_decoration {
    continue;
}
```

### Why `term_grid` and `render_term` carry duplicated SGR logic

Two consumers, ~50 LoC each. Extracting into a shared helper buys
one location to edit instead of two; we'd pay with: a new public
API surface on `term_gpu`, signature decisions about what to pass
in (palette? font system? scale factor?), and a less self-contained
example. YAGNI says wait for a third consumer. When `anyclaude`
itself starts rendering through `term_gpu` (Phase 5), that's the
third consumer and the natural extraction point.

## 17. Scrollback in `term_grid` (Phase 6 partial)

Six functional commits + one revert + one fix.
`scroll_demo` (Phase 3.5 prototype) already had the momentum
integrator and the wheel-event plumbing; this work was port plus
multi-panel and follow mode.

### Snapshot grew

`RenderSnapshot.rows` previously cloned just the visible region.
Rendering scrollback needs every row, so the field grew to hold
the full buffer (scrollback first, then visible) alongside a new
`visible_rows: usize`. Helpers `visible_start()` and
`visible_iter()` keep existing consumers minimal:

```rust
pub struct RenderSnapshot {
    pub rows: Vec<Row>,           // ALL rows now (was visible-only)
    pub visible_rows: usize,       // count of trailing visible rows
    pub cursor: CursorState,
    pub title: String,
    pub cwd: Option<String>,
}
```

`render_term` and the `dump` example switched to `visible_iter()`
so their behavior is unchanged. `term_grid` walks
`snapshot.rows.iter()` directly with the scroll offset applied
per row.

### Per-panel ScrollState, app-level in-flight gesture

```rust
struct PanelState { /* ... */ scroll: ScrollState }

struct App {
    /* ... */
    scrolling_panel: Option<PanelId>,
    scroll_velocity: Option<ScrollVelocity>,
    momentum_abort: Option<AbortHandle>,
    gesture_end_abort: Option<AbortHandle>,
}
```

Only one panel has inflight momentum at a time. Switching panels
(by hovering a new one with a wheel event) discards the previous
velocity sample and points the abort handles at the new target.
`CustomEvent::MomentumTick(PanelId)` carries the panel id so a
stale tick after focus change can be dropped:

```rust
CustomEvent::MomentumTick(id) => {
    if self.scrolling_panel == Some(id) {
        self.on_momentum_tick();
    }
}
```

### Rendering: baseline + offset projection

```rust
let scroll_offset_y_physical = scroll_offset_y_logical * sf;
let total_rows = snapshot.rows.len();
let visible_rows = snapshot.visible_rows;
let baseline_offset_phys =
    (total_rows.saturating_sub(visible_rows)) as f32 * cell_h_phys;
for (row_idx, row) in snapshot.rows.iter().enumerate() {
    let row_y_phys = row_idx as f32 * cell_h_phys
        - baseline_offset_phys
        + scroll_offset_y_physical;
    if row_y_phys + cell_h_phys <= 0.0 || row_y_phys >= panel_max_y_phys {
        continue;
    }
    // ...render cells...
}
```

Note `continue`, not `break`. The loop now starts at row 0
(potentially far above the panel when scroll is near max), so the
first iterations skip; later iterations land in the panel.

### Follow mode: capture pre, act post

```rust
fn drain_panel(&mut self, id: PanelId) {
    self.refresh_scroll_geometry(id);
    let was_at_bottom = self.panels.get(&id)
        .map(|p| p.scroll.offset_y <= SCROLL_BOTTOM_EPSILON)
        .unwrap_or(true);
    // ...apply PTY bytes...
    if was_at_bottom {
        self.refresh_scroll_geometry(id);
        if let Some(panel) = self.panels.get_mut(&id) {
            panel.scroll.offset_y = 0.0;
        }
    }
}
```

`SCROLL_BOTTOM_EPSILON = 0.5` (logical px) swallows float
accumulation from wheel deltas. Users who scrolled up explicitly
keep their position — `was_at_bottom` is false, follow mode
doesn't engage.

### The convention divergence

`term_gpu::ScrollState` is documented with `offset_y == 0` at the
top of content. `term_grid` uses the opposite: 0 is at the BOTTOM
(cursor visible), `max_offset` is at the TOP of scrollback. The
flip is deliberate:

1. Default state of `ScrollState::default()` puts `offset_y = 0`,
   which under `term_grid`'s convention means "at the cursor".
   Matches the expected initial state of a fresh terminal.
2. macOS natural scrolling delivers positive `MouseScrollDelta`
   on the fingers-down gesture. `scroll_by(positive)` increases
   `offset_y` — under `term_grid`'s convention, that's "scroll up
   into scrollback". No manual sign inversion needed.

Mid-debug I "corrected" `populate_panel` to match the
ScrollState docs (`offset_y == 0` → render top of buffer). The
user reported the scroll felt inverted. The actual bug was in
`drain_panel`'s `was_at_bottom` check (it was
`offset_y >= max - eps`, the right side for ScrollState's
convention but the wrong side for `term_grid`'s). Reverting
`populate_panel` and fixing `was_at_bottom = offset_y <= eps`
restored both follow mode and the wheel direction.

The convention is now documented as a comment block in
`populate_panel`:

```rust
// Scroll convention:
//   * offset_y == 0           → BOTTOM (cursor visible)
//   * offset_y == max_offset  → TOP of scrollback
```

Lesson: a library's documented convention is what the library
was designed around, not a law for downstream callers. The fix
when something feels wrong isn't necessarily "align to the
library" — it's "make every part of the downstream code
consistent with itself".

## 18. Selection in `term_grid` (Phase 6 partial)

Three commits (`773d37b`, `6598d7f`, `d82418f`). Drag-to-select,
double-click word, triple-click row, Esc clears. No copy yet —
that's clipboard's job, next deliverable.

### Data model

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellPoint { row: usize, col: usize }  // absolute into RenderSnapshot::rows

#[derive(Debug, Clone, Copy)]
struct Selection { anchor: CellPoint, cursor: CellPoint }

struct PanelState {
    /* ... */
    selection: Option<Selection>,
}

struct App {
    /* ... */
    dragging_selection: Option<PanelId>,
    last_click: Option<LastClick>,  // for double/triple click detection
}
```

`row` is the absolute index into `RenderSnapshot::rows`
(scrollback + visible). Selection coordinates survive user scroll
without translation; the renderer's baseline + scroll-offset math
handles the visual projection.

### Lifecycle

Warp's `app/src/terminal/model/selection.rs:1-6` doc-comment
defines the rules:

> A selection should start when the mouse is clicked, finalized
> when the button is released, cleared when text is
> added/removed/scrolled on the screen, and cleared if the user
> clicks off.

Our implementation:

| Trigger | Action |
|---|---|
| Mouse-press inside a panel, `mouse_mode == None` | Start (linear / word / row depending on click count) |
| CursorMoved while dragging | Update `selection.cursor` |
| Mouse-release with `selection.is_empty()` | Clear |
| `drain_panel` (PTY bytes) | Clear unless this panel is mid-drag |
| `sync_panels_to_tree` with grid change | Clear (reflow shuffles rows) |
| Esc keypress | Clear (still forward to PTY) |
| User scroll (wheel, momentum, Cmd+Home/End) | KEEP (viewport-only change) |

The "mid-drag exception" inside `drain_panel` is the only
deviation from Warp's strict rule — without it a bursty shell
would kill in-progress gestures the moment a byte arrives.

### Mouse-mode gate

```rust
let owns_mouse = self.panels
    .get(&id)
    .map(|p| p.emulator.mouse_mode() != MouseMode::None)
    .unwrap_or(false);
if !owns_mouse {
    /* start selection */
}
```

When Vim / htop / fzf / mc are in mouse-reporting mode, their
drag goes through the PTY. Without the gate we'd shadow in-app
gestures (e.g. Vim's visual-block mode, htop's row click).

### Multi-click detection

```rust
const MULTI_CLICK_THRESHOLD_MS: u128 = 400;

fn bump_click_count(&mut self, panel: PanelId, point: CellPoint) -> u32 {
    let now = Instant::now();
    let new_count = match self.last_click {
        Some(lc)
            if lc.panel == panel
                && lc.point == point
                && now.duration_since(lc.time).as_millis() <= MULTI_CLICK_THRESHOLD_MS =>
        {
            if lc.count >= 3 { 1 } else { lc.count + 1 }
        }
        _ => 1,
    };
    self.last_click = Some(LastClick { time: now, panel, point, count: new_count });
    new_count
}
```

Threshold at 400 ms (macOS default is ~500); count wraps from 3
back to 1 so the fourth consecutive click starts a fresh linear
selection.

### Word expansion

Word boundary characters lifted verbatim from Warp's
`crates/warpui_core/src/text/words.rs::DEFAULT_WORD_BOUNDARY_CHARS`
— 33 punctuation chars plus whitespace. `expand_word` walks
left and right from the clicked cell while the boundary-class
matches:

```rust
let center_is_boundary = is_word_boundary(cells[point.col].c);
let mut start_col = point.col;
while start_col > 0 && is_word_boundary(cells[start_col - 1].c) == center_is_boundary {
    start_col -= 1;
}
let mut end_col = point.col;
while end_col + 1 < cells.len()
    && is_word_boundary(cells[end_col + 1].c) == center_is_boundary
{
    end_col += 1;
}
(CellPoint { row: point.row, col: start_col },
 CellPoint { row: point.row, col: end_col + 1 })
```

The "boundary class" approach (instead of "only word chars
extend") means clicking on a `;` selects the run of `;`s — same
behavior as Warp.

### Render

`push_selection_rects` emits one `RectInstance` per cell row of
the selection, color `[118/255, 167/255, 250/255, 0.4]` (Warp's
`text_selection_color`). Same baseline + scroll-offset math as
`populate_panel` so the highlight scrolls with content. Linear
(row-wrapping) selection only — block mode deferred.

The rect pass runs before glyphs (renderer architecture: all
rects, then all glyphs), so glyphs render on top of the
highlight and stay readable through the 0.4 alpha.

## 19. Clipboard — separate crate, NSPasteboard FFI, image paste

Seven commits, new sibling crate `term_clipboard` joining
term_core / term_gpu / term_layout. Full Warp parity for the
data model (plain_text + paths + html + images), macOS backend
(`NSPasteboard` via `objc2-app-kit`), and the paste decision
flow.

### Crate placement

```text
crates/
  term_core/       VT parser + grid
  term_gpu/        renderer + atlas + scroll
  term_layout/     BSP panels
  term_clipboard/  ← new
```

Warp puts clipboard in `warpui_core::clipboard`. We mirror that
intent — clipboard is platform integration, not rendering.
Surfacing it as its own crate keeps each crate's responsibility
clean and makes it reusable from a future anyclaude integration
without dragging in `term_gpu`.

### Data model = Warp's

```rust
pub trait Clipboard: Send + 'static {
    fn write(&mut self, contents: ClipboardContent);
    fn read(&mut self) -> ClipboardContent;
    fn write_to_primary_clipboard(&mut self, contents: ClipboardContent) { /* default → write */ }
    fn read_from_primary_clipboard(&mut self) -> ClipboardContent { /* default → read */ }
}

pub struct ClipboardContent {
    pub plain_text: String,
    pub paths: Option<Vec<String>>,
    pub html: Option<String>,
    pub images: Option<Vec<ImageData>>,
}

pub struct ImageData {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub filename: Option<String>,
}
```

Identical to `crates/warpui_core/src/clipboard.rs`. Helpers
`is_empty`, `has_image_data`, `num_paths`,
`has_non_image_filepaths`, plus the
`should_insert_text_on_paste` heuristic, are all ports.

### NSPasteboard via objc2-app-kit, not arboard

Project pattern across the board: hand-rolled VT parser, custom
grid, BSP layout from scratch. Adding `arboard` (the popular
cross-platform clipboard crate) would have broken consistency
just for ~3 LoC of integration. `objc2-app-kit` is already in
our dependency tree (winit pulls it in for window management);
making it explicit costs a Cargo.toml line.

The macOS backend is ~170 LoC. Plain text write uses
`writeObjects` with `NSString::copy()` wrapped in
`ProtocolObject<dyn NSPasteboardWriting>`. HTML and images use
the lower-level `addTypes_owner` + `setString_forType` /
`setData_forType` so they layer onto the same pasteboard item
as the plain text. Image MIME ↔ NSPasteboard UTI mapping
matches Warp:

```rust
"image/png" => Some("public.png"),
"image/jpeg" | "image/jpg" => Some("public.jpeg"),
"image/gif" => Some("public.gif"),
"image/webp" => Some("public.webp"),
"image/svg+xml" => Some("public.svg-image"),
```

File-path reading goes through `readObjectsForClasses:options:`
with the `NSURL` class — the pattern in
`objc2-app-kit-0.2.2/examples/nspasteboard.rs::get_text_2`:
cast the class pointer through `AnyObject` to satisfy
`NSArray`'s element type, retain, then read each result's
`path` via `NSURL::path`.

### Tests + the SIGSEGV trap

InMemoryClipboard and the helpers have 15 ordinary integration
tests. The macOS round-trip is a single `#[ignore]`-gated test
function holding every scenario (plain text, empty no-op,
unicode, HTML coexistence, image data with caption).

The reason it's one function: parallel access to NSPasteboard
from multiple non-main test threads SIGSEGVs reliably. cargo's
test runner spawns one thread per `#[test]`, so two functions
would race. We don't have a `serial_test`-style crate, so
folding all scenarios into one function trades a longer
`#[test]` body for zero extra deps. `#[ignore]` keeps stock
`cargo test` from trashing the user's pasteboard.

### Paste decision flow follows Warp's process_paste_event

`paste_into_focused` in `term_grid.rs` mirrors Warp's
`process_paste_event` (`app/src/terminal/input.rs:10573`):

```rust
fn paste_into_focused(&mut self) {
    let content = self.clipboard.read();
    let mut parts: Vec<String> = Vec::new();

    if should_insert_text_on_paste(&content) && !content.plain_text.is_empty() {
        parts.push(content.plain_text.clone());
    }
    if let Some(paths) = content.paths.as_deref() {
        for path in get_image_filepaths_from_paths(paths) {
            parts.push(shell_quote_path(&path));
        }
    }
    if let Some(images) = content.images.as_deref() {
        if let Some(best) = pick_best_image(images) {
            if let Some(path) = save_image_to_temp(best) {
                parts.push(shell_quote_path(&path));
            }
        }
    }
    /* join with space, encode_paste, write to PTY */
}
```

Order matters: text first, then image filepaths, then saved-image
paths. This is Warp's order. Reversing it would change how
mixed payloads land in the PTY.

`pick_best_image` walks `CLIPBOARD_IMAGE_MIME_TYPES` (png →
jpeg → jpg → gif → webp) and returns the first match. Same
priority Warp uses.

`save_image_to_temp` writes to
`$TMPDIR/term_grid_clipboard_<nanos>.<ext>`. This is what makes
Claude Code's image input work: a screenshot copied via
Cmd+Shift+Ctrl+4 lands as PNG data on the pasteboard; Cmd+V
writes the PNG to /tmp and pastes the path into the CC chat
input. CC reads the file. Temp files leak — cleanup is a
follow-up polish item.

### Layout-agnostic shortcuts

Late discovery: Cmd+C on a Russian keyboard layout produced
`Key::Character("с")` (Cyrillic), and our match against `"c"`
missed silently. Fix is universal: match on
`event.physical_key`:

```rust
if let PhysicalKey::Code(code) = event.physical_key {
    match code {
        KeyCode::KeyC => self.copy_focused_selection(),
        KeyCode::KeyV => self.paste_into_focused(),
        KeyCode::KeyD => /* split */,
        KeyCode::KeyW => /* close */,
        KeyCode::KeyQ => /* exit */,
        _ => {}
    }
}
```

macOS apps universally use hardware-position shortcuts. We
didn't realize until the user reported "copy doesn't work on
Russian layout"; extended to every Cmd combo at the user's
explicit request.

## 20. Glyph cache fast-path — direct cmap for ASCII

Two commits, the structural performance pass. The audit was
short: `TextShapeCache::shape(text: &str, ...)` was building
its cache key with `text.to_string()` on every call — a
`String` allocation per cell, even on cache hits. At
200×60 cells × 60 fps that's ~720 000 allocations per second
the allocator was doing for nothing.

### Warp's solution, in their words

Per the user's "смотри на warp, как на эталон" line, the
first move was to read Warp's text path. `CellGlyphCache`
in `app/src/terminal/grid_renderer/cell_glyph_cache.rs:16`
opens with a comment that names the problem directly:

> We have 2 separate caches internally for performance
> reasons (avoid allocating strings when we don't need to!)

The two caches:

```rust
pub struct CellGlyphCache {
    glyph_cache: HashMap<(char, FontId), Option<(GlyphId, FontId)>>,
    string_cache: HashMap<(String, FontId), Option<(GlyphId, FontId)>>,
}
```

`Cell::raw_content()` (`crates/warp_terminal/src/model/grid/
cell.rs:190`) returns either `CharOrStr::Char(c)` or
`CharOrStr::Str(&str)` based on whether the cell has any
zero-width modifiers stored. Warp's grid renderer
(`render_cell_glyph` at `grid_renderer.rs:1725-1744`)
dispatches on this enum: single chars take the fast path,
multi-codepoint strings take the slow path.

The fast path doesn't go through cosmic-text at all. On
Linux/Windows (`crates/warpui/src/windowing/winit/fonts.rs:
1219`):

```rust
fn glyph_for_char(&self, font_id: FontId, c: char) -> Option<GlyphId> {
    self.try_read_font_face(font_id, |font_face| {
        font_face.glyph_index(c).map(GlyphIdExt::to_glyph_id)
    })?
}
```

`font_face` is `ttf_parser::Face` — this is a direct cmap
lookup, no shape buffer, no BiDi analysis.

### Our adaptation

We use cosmic-text already, and cosmic-text re-exports
`ttf_parser` at `cosmic_text::ttf_parser`. `Font::rustybuzz()`
returns a `RustybuzzFace<'_>` that derefs to `ttf_parser::
Face<'a>`. So we can do exactly Warp's cmap call without
adding a dependency:

```rust
fn resolve_primary_face(
    font_system: &mut FontSystem,
    family: &FontFamily,
    weight: Weight,
    style: Style,
) -> Option<FaceInfo> {
    let query = fontdb::Query {
        families: &[family.as_cosmic()],
        weight,
        stretch: fontdb::Stretch::Normal,
        style,
    };
    let id = font_system.db().query(&query)?;
    let font = font_system.get_font(id)?;
    let face = font.rustybuzz();
    let upem = face.units_per_em() as f32;
    if upem <= 0.0 { return None; }
    let ascent_em = face.ascender() as f32 / upem;
    Some(FaceInfo { font_id: id, ascent_em })
}
```

That's the per-`(weight, style)` resolution, cached for the
lifetime of `TextShapeCache`. The per-char lookup is then
just `font.rustybuzz().glyph_index(ch)` against the resolved
face, cached by `(char, font_id)`.

### Choosing the atlas key without a layout pass

The atlas is keyed on cosmic-text's `CacheKey`, which has a
public constructor:

```rust
pub fn new(
    font_id: fontdb::ID,
    glyph_id: u16,
    font_size: f32,
    pos: (f32, f32),
    flags: CacheKeyFlags,
) -> (Self, i32, i32)
```

The `pos` is the *physical* position of the glyph's
top-left, and `CacheKey::new` does the SubpixelBin binning
internally — returning the cache key plus the floor of the
position so the renderer knows where to draw the rasterized
quad. By constructing the key this way at the fast-path
callsite, we get bit-identical keys to what
`LayoutGlyph::physical` would have produced, which means
glyphs rasterized via the slow path are reused by the fast
path and vice versa. No atlas churn, no double-rasterization
on font changes.

### The dispatch gate

`prepare_shape_for_panel` chooses fast vs slow per cell:

```rust
let zerowidth_count = cell.extra.as_ref().map_or(0, |e| e.zerowidth.len());
let mut fast_path_handled = false;
if zerowidth_count == 0 {
    if let Some(cg) = shape_cache.shape_char(
        font_system, cell.c, FONT_SIZE, sf, weight, style,
    ) {
        let font_size_physical = FONT_SIZE * sf;
        let baseline_y_phys = cell_origin_y_phys + cg.baseline_y_physical;
        let (cache_key, x_floor, y_floor) = CacheKey::new(
            cg.font_id,
            cg.glyph_id,
            font_size_physical,
            (cell_origin_x_phys, baseline_y_phys),
            CacheKeyFlags::empty(),
        );
        // … atlas lookup + push GlyphInstance
        fast_path_handled = true;
    }
}
if !fast_path_handled {
    // existing String-keyed slow path
}
```

`shape_char` returns `None` if either face resolution or
`glyph_index` returns `None` — those cases (e.g. a glyph
genuinely absent from the primary face) fall through to the
slow path, where cosmic-text's font fallback chain takes
over. Bold/italic still works on the fast path because face
resolution is keyed on `(weight, style)`.

### Verification discipline

We don't run a `cargo bench` — there's no scaffolding for
that and the qualitative measure is what the user cares
about. Verification was:

1. `cargo test --workspace` — ~250 tests green.
2. Run `term_grid`, type in a real shell, watch nothing
   change visually. ASCII text renders identically;
   combining marks and CJK still hit the cosmic-text
   fallback for shaping.
3. Pause for explicit user verification before committing
   docs/memory updates — `feedback_verify_before_docs.md`
   in practice.

### What we did NOT do

- **No criterion benchmark.** The allocations removed are
  measurable in principle (Instruments / `dtrace`) but the
  structural argument is what matters: the work the
  allocator was doing is provably gone, not hidden behind a
  smaller hash. Adding a benchmark crate would have been
  YAGNI noise at this stage.

- **No custom cmap parser.** cosmic-text already pulls in
  `ttf_parser` and exposes `Font::rustybuzz()`, which derefs
  to `ttf_parser::Face`. The cmap lookup is a one-liner; no
  reason to bypass it.

- **No removal of cosmic-text from the slow path.**
  Combining marks (e.g. zalgo text, IPA diacritics) and
  multi-codepoint clusters still need shaping. Warp keeps
  the same slow path for the same reason.
