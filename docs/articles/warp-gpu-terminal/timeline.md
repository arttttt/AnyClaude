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

## 8. Phase 1 — term_core (8 commits + dump example)

After the GPU stack worked end-to-end, time for the parser. Started
with a second delegated research pass against Warp to find what our
spec missed — turned up two material decisions before any code was
written. Then 8 atomic commits, all compiling, integration tests
green at the end:

| Commit | Adds |
|---|---|
| `492e34f` | `docs(analysis): add Warp VT parser research` — what Warp uses (`vte` fork), grid model (fixed-cell alacritty-style), 30+ sequences our spec was missing, P0/P1/P2/P3 priorities, what NOT to do |
| `337c081` | `docs(gpu-terminal): rewrite term_core spec for Cell grid + extended VT` — full §4 rewrite: Cell-based grid, added all P0/P1 sequences with Priority column, OSC 7/8/133, full DEC modes list, "not supported" expanded to match research §4 |
| `aa14950` | `feat(term_core): bootstrap crate with color, attrs, and Cell primitives` — `TermColor`/`AnsiPalette` (default_dark with 16+216+24 ramp), `CellFlags` (u16 bitset, 12 flags), `Cell { c, fg, bg, flags, Option<Box<CellExtra>> }`, `PromptMarker`. Zero deps. |
| `a710e21` | `feat(term_core): add Paul Williams VT parser state machine` — 770 LoC. State enum (Ground/Escape/CSI/OSC/DCS/SOS-PM-APC/UTF8), `Action` enum with 30+ variants, full CSI/SGR/OSC/ESC dispatch |
| `541a26b` | `feat(term_core): add fixed-cell grid with scroll region and alt screen` — `Row`, `Grid`, `CursorStyle` (6 variants matching DECSCUSR), `MouseMode`. All operations: print/erase/insert/delete/scroll/alt/save/restore/resize/reset. DECOM-aware `cursor_position` |
| `c9c6536` | `feat(term_core): wire parser actions into VtEmulator` — `TerminalEmulator` trait, `RenderSnapshot`, `VtEmulator`. `apply_action` dispatch + `apply_sgr`. DA replies `\x1b[?6c`, DSR cursor pos `\x1b[r;cR` |
| `d83c53d` | `feat(term_core): support full DEC mode set and shell integration OSCs` — DEC 12/1000/1002/1003/1004/1006/2004/2026, DECRQM replies, OSC 7 CWD, OSC 8 sticky hyperlinks, OSC 133 one-shot prompt markers, `emit_focus` helper |
| `dd60a29` | `test(term_core): integration tests for parser and emulator` — 39 tests in `crates/term_core/tests/` (parser_smoke 20 + emulator_smoke 19). All green. Caught and fixed the "tests in `src/`" policy violation along the way |
| `bbf6ea5` | `feat(term_core): add dump example for visual smoke testing` — `examples/dump.rs`: stdin → grid snapshot with ASCII frame + cursor/title/cwd/responses. `COLOR=1` re-emits SGR so the dump itself shows in colour |

## 9. Mini-integration — term_core × term_gpu (6 commits + 2 term_core fixes)

After both crates compiled in isolation, we wired them through a new
example to prove they actually fit together. Four planned commits
turned into six once two `term_core` bugs surfaced.

User-facing requirement was specific: "Warp scrolls smoothly and
reflows panels nicely; tmux feels rough." The mini-integration's
job was to confirm that the GPU rendering quality holds when fed by
real VT bytes through real `term_core` snapshots.

| Commit | Adds |
|---|---|
| `6e5a13c` | `feat(term_gpu): bootstrap render_term example with stdin pipe` — `term_core` as a dev-dep of `term_gpu`; reader thread; signal-via-`EventLoopProxy`; first render is a clear-colour pass |
| `5fe2f74` | `feat(term_gpu): render cell grid as per-cell shaped glyphs` — per-cell shaping, integer `cell_width = round(advance_M_phys)`, glyph X = `col × cell_width` (Warp parity, advances ignored), Y-snap on baseline, DPI propagation from `renderer.scale_factor()` |
| `67597a6` | `feat(term_gpu): render cell backgrounds, cursor, and INVERSE flag` — bg rects, cursor (Block/Underline/Bar), INVERSE swap |
| `8451fc2` | `fix(term_core): terminate Grid::resize when rows > visible_rows` — caught when `fit_grid_to_window` first called `resize(120, 33)`; previous loop bound `visible_start() + rows` recomputed `visible_start()` from mutated state on each push and never terminated |
| `ff7ae68` | `fix(term_core): make Grid::resize top-anchored` — user explicitly wanted "контент не двигается" (matches their Warp config); rewrote resize as truncate-bottom-on-shrink + pad-bottom-on-grow + cursor clamp; earlier attempts (preserve cursor absolute row, pull-from-scrollback) all failed visually |
| `bca9192` | `feat(term_gpu): propagate window resize into the emulator` — lazy `fit_grid_to_window` on the redraw path with a `grid_size` cache so a winit drag burst coalesces into one emulator resize per frame |

Two non-obvious bugs from this set deserve a callout in the article:

1. **The blur was DPI, not subpixel.** First user report after
   commit 2: "тексты слегка размыты". Three rounds of fixes
   (pixel-snap origin, then per-cell shaping, then Y-snap) made it
   *slightly* better but not crisp. The actual cause was a YAGNI
   regression in commit 1: I'd removed `self.scale_factor = renderer.scale_factor()` from `resumed()`
   because nothing in that commit consumed it. By commit 2 the
   shape calls used `self.scale_factor` (still `1.0`) while the
   framebuffer was Retina — glyphs got rasterised at logical pixel
   size and the GPU sampler bilinearly stretched them ×2. Restoring
   the field made the text crisp instantly. **Lesson** ([[feedback-solid-dry-kiss-yagni]]):
   YAGNI doesn't extend to fields with non-obvious downstream
   consumers — verify there's truly no consumer before deletion.

2. **Grid::resize hung on first window open.** `fit_grid_to_window`
   called `emulator.resize(120, 33)` (window 1920×1200 / cell 16×36).
   App froze for ~6 seconds, then OS killed it. Debug `eprintln`s
   showed control never returning from `fit_grid_to_window`. Looking
   at the loop:

   ```rust
   while self.rows.len() < self.visible_start() + rows {
       self.rows.push(Row::new(cols));
   }
   ```

   `visible_start()` returns `self.rows.len() - self.visible_rows`.
   Each push grows `rows.len()` by 1; `visible_start()` therefore
   grows by 1 too; condition stays true forever. Fix is to snapshot
   the loop bound: `let target = scrollback + rows;` before the
   loop. Classic "loop invariant mutated by loop body" — caught now
   by `tests/emulator_smoke.rs::resize_grows_grid_to_taller_visible_region`.

3. **Top-anchored is non-default but user-chosen.** A delegated agent
   research pass against Warp's actual `grid_storage/resize.rs` found
   the standard alacritty-style algorithm: shrink scrolls top rows
   into scrollback to keep the cursor anchored to the bottom; grow
   pulls them back. The user explicitly opted out:
   > "у меня варп настроен так, что контент внутри него ресайзится,
   > но не двигается вверх, вниз или куда либо еще"

   So `Grid::resize` ships with truncate-bottom semantics (content
   pinned to top, lost when shrunk past) instead of the alacritty
   default. Future work could expose this as a flag.

## 10. Phase 4 — term_layout (6 commits)

Last of the three core crates. Pure data structure — `Box<Node>` BSP
tree producing rectangles, consumers wire those into renderers,
input handlers, whatever. The data structure is well-trodden, but
hitting all of Warp's capabilities (split / close / resize / hit
test / drag dividers) and keeping commits atomic took some thought.

| Commit | Adds |
|---|---|
| `a7e8262` | `feat(term_layout): bootstrap crate with PanelTree primitives` — `PanelId`, `Rect`, `Split`, internal `Node`, `PanelTree::new` / `panels` / `focus` / `is_empty`. Zero deps. Workspace member added. |
| `48c3ecd` | `feat(term_layout): implement split and proportional resize` — these ship together because `resize` is the consumer that makes `Branch.{split,ratio,bounds}` load-bearing; splitting them would have required `#[allow(dead_code)]` which violates the project's no-scaffolding-without-a-consumer rule |
| `09d2ccc` | `feat(term_layout): implement PanelTree::close` — consume-recursive helper, sibling promoted via `recompute_bounds`, focus follows |
| `333bf31` | `feat(term_layout): implement PanelTree::hit_test` — half-open right/bottom edges so an exact divider pixel doesn't hit two panels |
| `721add7` | `feat(term_layout): implement dividers and drag_divider` — `BranchId` namespace separate from `PanelId` for stable drag handles; `Divider { id, split, rect }` returned for drawing/hit-test |
| `e24dc52` | `feat(term_layout): add set_focus + visual layout_demo example` — `set_focus(id) -> bool`; `Divider.bounds` added (the demo's drag handler needs parent bounds to compute new ratio); demo at `crates/term_gpu/examples/layout_demo.rs` with Cmd-key shortcuts, click-to-focus, mouse-drag divider |

Demo runs at 120 fps on Retina; window resize / split / close
animations are instant (no PTY in the loop). 28 integration tests
across six test files in `crates/term_layout/tests/`.

## 11. Branch state at end of Phase 4

47 commits on `feat/gpu-terminal` total. Three crates compile clippy
clean:

| Crate | LoC (src) | Tests | Deps |
|---|---|---|---|
| `term_core` | ~2000 | 22 | 0 |
| `term_gpu` | ~1300 | — (visual demos) | wgpu, winit, cosmic-text, futures, futures-timer, glam, pollster |
| `term_layout` | ~250 | 28 | 0 |

Three visual demos:

- `scroll_demo` — 120 fps pixel-scroll with Warp momentum
- `render_term` — `cat session.log | render_term` shows a real
  terminal grid rendered through cosmic-text
- `layout_demo` — split / close / drag panels with Cmd-key
  shortcuts

The full vertical stack works end-to-end. Phase 5 (integration into
the AnyClaude CLI) is the remaining work — but it requires a
non-trivial UX call (how panels map to Claude Code sessions, tab
semantics, header/footer chrome) which the user has chosen to
defer.

## 12. term_grid — the first real terminal (5 commits)

Combined demo: every leaf in the `PanelTree` owns a real shell
PTY. `portable-pty` spawns `$SHELL`, a reader thread per panel
pumps bytes through `EventLoopProxy<CustomEvent::BytesArrived(id)>`,
keyboard input is encoded to ANSI bytes and written to the focused
PTY, and divider drags resize both the visual layout and the
underlying shells.

| Commit | Adds |
|---|---|
| `30dc67b` | `feat(term_gpu): bootstrap term_grid example with a single PTY shell` — portable-pty as a dev-dep, single panel, reader thread, shell prompt appears on first frame |
| `b1f6955` | `feat(term_gpu): route keyboard input to the focused PTY in term_grid` — `encode_key` covers printable text, named keys (Enter/Tab/arrows/Home/End/Delete/PageUp/Down), `Ctrl + letter` ASCII control codes, `Alt + key` ESC-prefix Meta; emulator responses (DA/DSR) flow back to the PTY |
| `fd70cbf` | `feat(term_gpu): multi-panel term_grid with split/close/focus/drag` — Cmd+D / Cmd+Shift+D / Cmd+W shortcuts, click-to-focus, drag-divider, slim focus border, `PanelExited(id)` event for shell-exit cleanup, exit-when-last-panel-closes |
| `e2a83ee` | `feat(term_gpu): propagate per-panel resize to emulator + PTY` — `sync_panels_to_tree` with `grid_size` cache; deferred until `on_mouse_release` for drags; render-side culling so glyphs/cursor don't spill past the panel during drag |
| `eb31fc4` | `docs(spec): list reflow in Phase 6 roadmap` — surfaced by user testing: shrinking columns truncates content forever, growing back shows blanks. Adds alacritty-style reflow as a concrete Phase 6 deliverable. |

Three lessons that belong in the article:

1. **SIGWINCH spam during drag.** First version called
   `sync_panels_to_tree` on every `CursorMoved`. Continuous drag fired
   dozens of resize events per second. zsh re-renders its prompt on
   each SIGWINCH; combined with our destructive shrink (`row.resize(
   5)` truncates cells past 5), the result was a left panel filled
   with partial prompts — "artem", "artem", "@Arte", "ms-Ma", …
   stacked from drag history. Fix: defer the sync to `on_mouse_release`.
   Tree mutates immediately (visual), shell sees one SIGWINCH at the
   end (semantic).

2. **The PanelTree's bounds shrink before the emulator knows.**
   Because the drag deferral above, during a drag the PanelTree's
   rect shrinks immediately but the emulator stays at its pre-drag
   dimensions. Without render-side culling, the (still-large) glyph
   grid spilled into the neighbouring panel. Fix: in `populate_panel`
   skip cells whose `col × cell_width_phys` exceeds the panel's
   physical width; same idea in `build_cursor_rect`. The lag between
   "tree bounds" and "emulator bounds" is a normal transient state,
   not a bug to chase.

3. **Reflow is not optional in a real terminal — but it's not
   simple.** After release, the shrink-and-grow round-trip dropped
   content (cells past the new width were gone). Tmux and alacritty
   wrap long lines on column shrink (with a continuation marker) and
   unwrap on grow; alacritty's `grid_storage/resize.rs::shrink_cols`
   / `grow_cols` is ~130 LoC and needs a per-row wrap flag we don't
   have yet. We accepted destructive resize for the demo, named the
   limitation explicitly, and added reflow to the Phase 6 roadmap.

## 13. Branch state at end of `term_grid`

53 commits on `feat/gpu-terminal`. Three crates + four demos:

| Demo | What it proves |
|---|---|
| `scroll_demo` | Pixel-scroll + momentum |
| `render_term` | term_core × term_gpu single-panel pipe |
| `layout_demo` | term_layout BSP shape + drag |
| `term_grid` | All three crates + per-panel PTY shells |

`term_grid` is the first end-to-end virtual terminal — open the
window, get a shell prompt; type, get output; `Cmd+D`, get a second
shell. It runs at 120 fps on Retina with multiple shells active
simultaneously. Phase 5 (anyclaude integration) and Phase 6
(polish: reflow, SGR visual flags, selection, clipboard,
scrollback, performance pass) remain.

## 14. Reflow on column resize (3 commits)

Phase 6 partial. `term_grid` was leaving "history fragments" after
drag-resize — `Grid::resize` truncated cells past the new column
count, leaving partial copies of the previous prompt on screen.
Reflow fixes this by rewrapping content via a per-cell soft-wrap
marker.

| # | Commit | Lines | Why |
|---|---|---|---|
| 55 | `4e5c5e2` | `crates/term_core/src/attrs.rs` | Add `CellFlags::WRAPLINE` (bit 12). |
| 56 | `901ed78` | `crates/term_core/src/grid.rs`, `crates/term_core/tests/emulator_smoke.rs` | `Grid::print` sets WRAPLINE on the auto-wrap branch; two smoke tests pin the contract. |
| 57 | `e2a4c4b` | `crates/term_core/src/grid.rs`, `crates/term_core/tests/reflow.rs` | Reflow algorithm (~80 LoC helpers) + 12 integration tests. |

Three lessons stuck:

1. **Cell-level flag beats per-Row field.** First plan had
   `Row.wrapped: bool`; switched after reading Warp's
   `FlatStorage::add_row` — it tests
   `row[cols-1].flags().intersects(WRAPLINE)`. The flag lives on a
   different cell than the one being overwritten, so it survives
   cell mutation. No extra `Row` state needed.

2. **Cursor by absolute row, not visible-relative, across multi-step
   resize.** Mid-resize `visible_start` shifts because `rows.len()`
   changes. A visible-relative cursor mid-flow lands on the wrong
   row. Track `cursor_abs` as a local, project to visible at the
   very end.

3. **Drop trailing all-blank logical lines before re-wrap.**
   First test pass had `helloworld` ending up in scrollback because
   the empty rows below the cursor in the source buffer became real
   rows in the rewrapped output, pushing visible_start down. The
   outer pad-with-blanks step recreates trailing blanks already —
   re-emitting them is double-counting.

## 15. Branch state at end of reflow

57 commits on `feat/gpu-terminal`. 56 integration tests in
`term_core` alone (24 + 20 + 12). `term_grid` drag-resize now
preserves content cleanly. Phase 6 remaining: SGR visual flags,
selection, clipboard, scrollback navigation, font fallback,
performance pass.

## 16. SGR visual flags (4 commits)

Phase 6 continued. Emulator was already setting the `CellFlags`
bits for BOLD / ITALIC / UNDERLINE / DOUBLE_UNDERLINE / STRIKE /
FAINT / HIDDEN; the renderer hardcoded `Weight::NORMAL` and ignored
all decoration flags. Four atomic commits closed the gap.

| # | Commit | Files | Why |
|---|---|---|---|
| 58 | `79da3d7` | `crates/term_gpu/src/text.rs`, `lib.rs` + 3 callsites | `TextShapeCache::shape` gains `(Weight, Style)`. Re-exports them from `term_gpu` so consumers don't depend on cosmic-text. All callsites pass `Weight::NORMAL` + `Style::Normal` — no behavior change. |
| 59 | `3b704e9` | `crates/term_gpu/examples/term_grid.rs` | `populate_panel` derives `Weight`/`Style` from `cell.flags.bold()/italic()`. Cosmic-text resolves bold/italic faces from the system font database. |
| 60 | `835d680` | `crates/term_gpu/examples/term_grid.rs` | UNDERLINE / DOUBLE_UNDERLINE / STRIKE as `RectInstance`s at cell-height fractions (0.78 / 0.72-0.84 / 0.42). FAINT multiplies fg alpha by 0.5. HIDDEN suppresses glyph but keeps bg + decorations. Blank-cell short-circuit gates on decoration flags too. |
| 61 | `675c92d` | `crates/term_gpu/examples/render_term.rs` | Same SGR logic ported into `render_term`. Two consumers; DRY threshold not reached so we accept the duplication (YAGNI). |

Two findings worth recording:

1. **Bold/italic via face switching, not synthesis.** Cosmic-text's
   `Attrs::weight(Weight::BOLD)` and `style(Style::Italic)` route
   through fontdb to system faces (SF Pro Bold / Italic on macOS).
   We don't synthesize bold by stroking glyphs or fake italic via
   shader skew — the system font database supplies real faces, and
   cosmic-text's CacheKey already includes weight/style so the
   atlas caches them as distinct glyphs naturally.

2. **Decorations live in the rect pass, not the glyph pass.** A
   bold underlined fg-cyan `'h'` is one glyph image and one
   underline rect, not a baked underline rasterized into the glyph.
   This keeps the atlas small (no underline-variant cache key) and
   the lines crisp at any DPI. Same logic for strike and
   double-underline. The decoration color is the cell's effective
   fg (already faint-attenuated if FAINT), so palette and INVERSE
   flow through naturally.

## 17. Branch state at end of SGR flags

61 commits on `feat/gpu-terminal`. All four demos render full SGR:
bold, italic, underline, double-underline, strike, faint, hidden.
Verification was light-touch (no clipboard yet → hard to paste
SGR test strings) so the SGR visual tuning may need a follow-up
once an interactive smoke pass is possible. Phase 6 remaining
(no fixed order): selection, clipboard, scrollback in `term_grid`,
font fallback, performance pass.

## 18. Scrollback in term_grid (6 commits + 1 revert + 1 fix)

The next user-visible Phase 6 item. The momentum integrator already
existed in `scroll_demo` (Phase 3.5 prototype); this work was port
+ multi-panel integration + follow-mode + the convention bug.

| # | Commit | Files | Why |
|---|---|---|---|
| 62 | `0d8b23b` | `term_core/src/emulator.rs`, `grid.rs`, +consumers | `RenderSnapshot.rows` now holds the full buffer; `visible_rows` and `visible_iter()` ride alongside it. |
| 63 | `ef15d9f` | `term_gpu/examples/term_grid.rs` | `PanelState` gains `scroll: ScrollState`. Wheel handler hit-tests against the panel tree and routes deltas. Single global momentum / gesture-end abort handles, keyed by panel id via `CustomEvent::{MomentumTick, GestureEnded}(PanelId)`. |
| 64 | `2b88388` | `term_gpu/examples/term_grid.rs` | `populate_panel` + `build_cursor_rect` take `scroll_offset_y` and apply a baseline + offset transform so all rows position correctly under any scroll. |
| 65 | `88426e9` | `term_gpu/examples/term_grid.rs` | Follow mode: snapshot `was_at_bottom` before applying bytes; re-pin if so. |
| 66 | `c23c26e` | `term_gpu/examples/term_grid.rs` | `Cmd+Home` / `Cmd+End` jumps. |
| 67 | `e56a33e` | `term_gpu/examples/term_grid.rs` | **Attempted fix** — switched populate_panel to `ScrollState`'s documented convention (0 = top). |
| 68 | `5700301` | `term_gpu/examples/term_grid.rs` | Revert of `e56a33e` after user feedback ("ты похоже скролл инвертировал"). |
| 69 | `c5ebc1b` | `term_gpu/examples/term_grid.rs` | Real fix — `was_at_bottom` check was inverted relative to `term_grid`'s convention; Cmd+End / Cmd+Home jump destinations swapped. |

Lessons:

1. **Per-snapshot rendering can absorb the scroll position math.**
   No new scroll uniform per panel, no separate render pass. The
   data crate (`term_core`) is unchanged; the renderer translates
   row indices to physical Y per cell, including the scroll offset.
   Multi-panel just works.

2. **One in-flight gesture, keyed by panel id.** Per-panel
   momentum threads would be overengineering for a UX where users
   scroll one thing at a time. A single `App.scrolling_panel:
   Option<PanelId>` and a single `momentum_abort` handle are
   enough. The `CustomEvent::MomentumTick(PanelId)` payload lets
   stale ticks be dropped cleanly when focus moves.

3. **Convention divergence from a library doc is fine — but
   commit to it.** The `term_gpu::ScrollState` doc says
   `offset_y == 0` is the top. `term_grid` flips this: 0 is the
   bottom, max is the top. The flip is what makes natural macOS
   scrolling work without sign inversion AND keeps the default
   state ("at cursor") at `offset_y = 0`. The mistake was
   "correcting" populate_panel to match the docs (e56a33e)
   without realizing the wheel direction was tuned to the
   inverted convention. The fix wasn't to change `populate_panel`
   — it was to align `was_at_bottom` and the jump destinations
   to the same convention the renderer was already using. The
   convention is now documented as a comment block in
   `populate_panel`.

4. **Capture user state PRE-change, act on it POST.** Follow mode
   snapshots `was_at_bottom` before processing bytes. If we
   checked after, the new `max_offset` would erase the signal.
   Same pattern works for "auto-anything when state shifts".

## 19. Branch state at end of scrollback

69 commits on `feat/gpu-terminal`. `term_grid` now usable as a
real terminal — long shell output (`man bash`, `seq 1 1000`,
`ls /usr/bin`) scrolls cleanly, momentum matches scroll_demo's
feel, and follow mode keeps the cursor visible while the shell
prints. Phase 6 remaining: selection, clipboard, font fallback,
performance pass.

## 20. Selection (3 commits)

Drag-to-select, double-click word, triple-click line, Esc clears.
No copy yet — that's clipboard's job. Coordinates absolute (row
index into `RenderSnapshot::rows`) so the highlight stays on its
content as the user scrolls.

| # | Commit | Files | Why |
|---|---|---|---|
| 70 | `773d37b` | `term_grid.rs` | `CellPoint`, `Selection` types on `PanelState`. `cell_at_panel` hit-test. Mouse-press / cursor-moved / release handlers. Clearing on PTY bytes (with mid-drag exception) + on column-resize. `push_selection_rects` for the highlight overlay. |
| 71 | `6598d7f` | `term_grid.rs` | `Esc` clears focused panel's selection (still forwarded to the PTY). |
| 72 | `d82418f` | `term_grid.rs` | Double-click → word, triple-click → row. Word boundary chars verbatim from Warp's `DEFAULT_WORD_BOUNDARY_CHARS`. |

Three lessons:

1. **Absolute row indices, not visible-relative.** Selection
   coordinates point into `RenderSnapshot::rows` directly. When
   the user scrolls the viewport, the indices don't move — the
   render-side baseline + scroll-offset math handles the
   projection. The alternative (visible-relative) would shift
   the highlight on every scroll event, which is wrong.

2. **Clear on text change, keep on viewport change.** Warp's
   `selection.rs` doc-comment names it explicitly: "cleared when
   text is added/removed/scrolled". Our `drain_panel` clears the
   selection after applying PTY bytes — with one carveout: the
   panel currently being dragged keeps its selection so a burst
   of shell output doesn't kill an in-progress gesture.
   `sync_panels_to_tree` also clears on a column or row change
   because reflow shuffles rows. User scroll, by contrast, leaves
   the selection alone.

3. **Mouse-mode gate.** Selection only starts when the emulator's
   `mouse_mode()` is `MouseMode::None`. When Vim / htop / fzf /
   mc are in mouse-reporting mode, their drag goes through the
   PTY instead. Without this gate we'd shadow in-app gestures
   and break selection inside Vim, scroll inside htop, etc.

## 21. Branch state at end of selection

73 commits on `feat/gpu-terminal`. `term_grid` now supports
drag-to-select with translucent blue highlights matching Warp's
color. Double-click selects words via Warp's
`DEFAULT_WORD_BOUNDARY_CHARS` list, triple-click selects rows.
Without clipboard yet the selection can't be copied out — that's
the next deliverable. Phase 6 remaining: clipboard, font fallback,
performance pass.

## 22. Clipboard — new crate + Cmd+C/V with full Warp parity (7 commits)

Phase 6 continued. New sibling crate `term_clipboard` joins
term_core / term_gpu / term_layout. Full functional parity with
Warp's clipboard module: plain text, HTML, file paths, and
images. Image paste lands as temp-file paths so Claude Code's
image input works through Cmd+V.

| # | Commit | Files | Why |
|---|---|---|---|
| 73 | `abf16f9` | `term_clipboard/{Cargo.toml,src/lib.rs,tests/in_memory.rs}` | Cross-platform crate skeleton: `Clipboard` trait + `ClipboardContent` + `ImageData` + `InMemoryClipboard` + `should_insert_text_on_paste` heuristic. 11 tests. |
| 74 | `f11561d` | `term_clipboard/src/mac.rs`, `tests/mac_smoke.rs` | `MacClipboard` via `objc2-app-kit::NSPasteboard`, plain text only. One `#[ignore]`-gated round-trip smoke. |
| 75 | `6e36d85` | `term_clipboard/src/mac.rs`, `tests/mac_smoke.rs` | Extend `MacClipboard` to HTML / images / file paths. Image MIME ↔ NSPasteboard UTI mapping. `readObjectsForClasses_options(NSURL)` for paths. |
| 76 | `a68e174` | `term_gpu/Cargo.toml`, `examples/term_grid.rs` | `App.clipboard: Box<dyn Clipboard>` (MacClipboard on macOS, InMemoryClipboard fallback). `selection_to_text` warp-style (trim trailing blanks, WRAPLINE → no newline). Cmd+C wired. |
| 77 | `e4563fe` | `term_gpu/examples/term_grid.rs` | Cmd+V handler reading plain text + bracketed-paste wrapping. ALL Cmd shortcuts switched to `event.physical_key` so they survive non-English keyboard layouts. |
| 78 | `048c55d` | `term_clipboard/src/lib.rs`, `mac.rs`, `tests/in_memory.rs` | Bring `term_clipboard` to full Warp utility parity: `IMAGE_EXTENSIONS` (5 entries to match), `CLIPBOARD_IMAGE_MIME_TYPES` priority list, `has_image_extension` made `pub`, `get_image_filepaths_from_paths`. JPEG MIME accepts both `image/jpeg` and `image/jpg`. |
| 79 | `cacf9f3` | `term_gpu/examples/term_grid.rs` | Cmd+V now follows Warp's `process_paste_event` fully: plain text + image filepaths from `content.paths` + best image data saved to `$TMPDIR/term_grid_clipboard_<nanos>.<ext>` and path appended. Shell-quoted paths so spaces survive. |

Four lessons stuck:

1. **Custom over crate, even when arboard would be 3 lines.**
   The project pattern is "write our own" — hand-rolled VT
   parser, custom cell grid, BSP layout from scratch, no
   ratatui / alacritty_terminal / crossterm. Adding `arboard`
   would have broken that consistency. `objc2-app-kit` was
   already in the tree (winit pulls it in); making it explicit
   cost a Cargo.toml line.

2. **macOS NSPasteboard is single-threaded for testing.**
   Parallel access from multiple non-main test threads
   SIGSEGVs reliably. We don't have a `serial_test`-style
   crate, so the workaround is "one `#[test]` function holds
   all the round-trip scenarios". Tests stay `#[ignore]`-gated
   so a stock `cargo test` doesn't trash the user's
   clipboard.

3. **Image paste = save-to-temp + paste-path.** A terminal
   can't accept raw image bytes — shell stdin is a byte
   stream. iTerm, Warp, and us all bridge clipboard images by
   writing the data to a temp file and pasting the path. The
   side-effect cleanup is left for later (temp files leak
   today).

4. **Layout-agnostic shortcuts via physical key.** Cmd+C on
   a Russian / French / Greek keyboard layout gives
   `Key::Character("с"|"ç"|"ψ")` — the `logical_key` match
   misses. Switching to `event.physical_key` and matching
   `KeyCode::KeyC` etc. anchors the shortcut to the hardware
   key, which is how macOS apps universally do it. Applied to
   every Cmd combo, not just C/V — the user pointed out
   "fix all the hotkeys".

## 23. Branch state at end of clipboard

78 commits on `feat/gpu-terminal`, 4 crates. `term_grid` now
covers the full keyboard / mouse / clipboard surface a real
terminal needs: type, scroll with momentum, select with
drag/double/triple-click, copy and paste plain text, paste
images via the temp-file bridge. Phase 6 remaining: font
fallback config, performance pass (direct codepoint→glyph_id).

## 24. Glyph cache fast-path — direct cmap for single-codepoint cells (2 commits)

User reminder after clipboard landed: "вспомнил, нужно
проверить производительность." Audit surfaced the culprit
quickly — `TextShapeCache::shape` was allocating a fresh
`String` for the cache key on every call (even cache hits).
At 200×60 cells × 60 fps that's ~720 000 allocations per
second just for cache lookups, and the slow path was running
unconditionally for every cell.

Research first, per the user's "смотри на warp, как на
эталон" line. The Explore agent surfaced Warp's structural
solution: `CellGlyphCache` (in `app/src/terminal/grid_renderer/
cell_glyph_cache.rs:16`) holds **two** caches — `glyph_cache:
HashMap<(char, FontId), Option<(GlyphId, FontId)>>` for
single-codepoint cells, `string_cache: HashMap<(String,
FontId), …>` only for combining clusters. The hot path uses
`font_face.glyph_index(char)` directly via `ttf_parser` —
no cosmic-text shaper, no allocation, no BiDi. The comment
in Warp says it out loud: *"avoid allocating strings when we
don't need to!"*

The path we'd designed was already aligned, so two atomic
commits:

- `3aa2a33` `perf(term_gpu): add char-keyed fast-path API
  to TextShapeCache`. New `CharGlyph { font_id, glyph_id,
  baseline_y_physical }` struct + `shape_char(font_system,
  ch, font_size, scale_factor, weight, style) -> Option
  <CharGlyph>` method. Key is `(char, font_id)`, no `String`.
  On miss, resolve primary face via `FontSystem::db().query
  (&fontdb::Query)` (one-time per `(weight, style)`),
  query `Font::rustybuzz().glyph_index(ch)`. `baseline_y_
  physical` derived from `ascender / units_per_em` cached on
  the face. ~160 LoC.

- `e67b7c2` `perf(term_gpu): route single-codepoint cells
  through shape_char fast-path`. `prepare_shape_for_panel`
  picks fast vs slow path: `cell.extra.zerowidth.is_empty()`
  → `shape_char` + manual `CacheKey::new(font_id, glyph_id,
  font_size_physical, (cell_origin_x, baseline_y),
  CacheKeyFlags::empty())` → atlas. Combining clusters fall
  through to the existing String-keyed slow path. ~80 LoC.

`CacheKey::new` returns the same atlas key that
`LayoutGlyph::physical` produces, including SubpixelBin
binning — so rasterized glyphs are shared between paths.
Bold/italic still work because face resolution keys on
`(weight, style)`.

Verification was the same pattern we settled on:

1. `cargo test --workspace` — ~250 tests, all green.
2. Run `term_grid` example, type in a real shell, check
   that nothing changed visually. The 99% case (`ls`,
   `cat`, prompt text) now bypasses the shaper entirely;
   the user shouldn't notice anything except maybe
   slightly steadier frame timing under heavy updates.
3. Pause for user verification ("работает / не работает")
   before docs/memory commits land — per the discipline
   we encoded earlier.

The win is structural, not just numerical: we now have the
same hot path as Warp for terminal grid rendering. Whatever
allocator activity remains on the render thread isn't
coming from the shape cache key.

## 25. Branch state at end of glyph cache fast-path

81 commits on `feat/gpu-terminal`, 4 crates. `TextShapeCache`
now has two tiers (char + string) matching Warp's
`CellGlyphCache.glyph_cache` vs `string_cache` split. Hot
render path for ASCII text is alloc-free on cache hit and
one cmap lookup on miss — no `cosmic_text::Buffer` for the
common case. Phase 6 remaining: font fallback configuration,
drop-shadow shader for overlays.

## 26. Phase 5 — anyclaude GPU integration (~30 commits)

**Setup half (C1-C9 + C10a):** Hidden `--gpu` flag routed to a
fresh `src/ui/gpu/` module while the legacy ratatui path stayed
alive next door. Skeleton → shell PTY rendering → keyboard /
scroll / selection / clipboard parity with `term_grid` → top
header + bottom footer chrome → drop-shadow shader → three popup
overlays (backend switch / history / settings). Each commit was a
tight diff because the heavy lifting had shipped in earlier
phases. Verification incremental thanks to the flag.

C10a ported ~250 LoC of legacy `runtime.rs` bootstrap (Config,
DebugLogger, tokio runtime, ProxyServer + try_bind, TeammateShim,
subagent hooks, proxy as tokio task) into `gpu::run`. `ChildPty`
started spawning claude with the real env. Backend popup pre-
selected the active backend; Enter called real `switch_backend`.
History pulled real switch log.

**MVI mid-stream refactor.** User feedback: "ВЕСЬ ui должен быть
на mvi". The popup state had been inline `Option<Popup>` enum +
field-mutating handlers. One refactor commit replaced it with
`Store<BackendSwitchActor>` / `Store<HistoryActor>` /
`Store<SettingsActor>` using the existing actor implementations
from the legacy ratatui path. Lesson: when a codebase has an
architecture convention, follow it even when "simpler" looks
possible.

**Warp parity fix half (FIX-1 .. FIX-4).** First screenshot of
running claude showed underlines under every line, stretched
alpaca logo, double-rendered title. Three iterations of explore
agents on Warp's source:

- **FIX-1**: cell metrics from real `ttf_parser::Face` ascent +
  descent + line_gap (Warp's `grid_size_util.rs:23-36`). Removed
  `LINE_HEIGHT_RATIO`.
- **FIX-2**: native painter `paint_block_char` for U+2580-U+259F
  block + shade characters — solid rects sized to integer cell
  pixels, not shaped glyphs (Warp's
  `render_native_glyph:2008+`). 32 block chars + 3 shades.
- **FIX-3**: non-sRGB swap chain (gamma-space blending) + luma-
  dependent glyph contrast (`k = dot(rgb, vec3(0.30, 0.59,
  0.11))`; `alpha *= (k+1) / (alpha*k + 1)`). Copied from
  Warp's `glyph_shader.wgsl` which lifted it from Windows
  Terminal's DirectWrite shader.

Block art tiled. Colors became saturated. Underlines remained.

Three more rounds of SGR-parser static analysis found real
adjacent bugs — `:` sub-param separator unhandled, SGR-4
dispatcher mis-consumed sub-args, alt-screen SGR state leaked.
Each correct. Underlines persisted.

**FIX-4 the breakthrough.** Fourth-round agent ran Claude Code
under `script -q -F` and captured the actual bytes
(`/tmp/claude_pty_trace.bin`). The trace said: claude never emits
plain `CSI 4 m` for the welcome screen. The previous three
parser fixes were chasing sequences that didn't exist. The real
culprit, sixth control sequence in the trace: `CSI > 4 ; 2 m` —
XTERM `modifyOtherKeys = 2`. Our `dispatch_csi` only treated `?`
as a private marker; for `>`, `<`, `=` it fell through to plain
SGR. Claude's extended-keyboard handshake at startup was being
dispatched as SGR 4;2 → permanent DOUBLE_UNDERLINE on every cell.

One commit (`f72f652`) — reject non-`?` private markers, reset
`param_is_sub` array in `reset_for_escape` — and the underline
went away.

**The workflow lesson, saved as memory**: when a
terminal-rendering bug looks like wrong attributes, capture PTY
bytes via `script` BEFORE static analysis. Three rounds of
parser-grepping missed what one trace made obvious.

## 27. Branch state at end of FIX-4

~110 commits on `feat/gpu-terminal`, 5 crates (mvi preserved per
user mandate). GPU UI runs Claude Code end-to-end. Five visible
bugs remain (title double-render, cursor placement, popup section
split, Cmd+R wiring, header sub/team labels) — tracked in
`gpu-terminal-remaining-bugs.md` for next-session pickup.
Cutover (delete ratatui paths, remove `--gpu` flag) deferred
until those settle.

## 28. Phase 5 closing pass — popup polish, then two UTF-8 / inverse root causes

One session. Eleven commits. Started with seven visible bugs;
ended with zero.

**Wave 1 — popup polish (FIX-5..7).** The MVI state for the
backend popup already carried `section` / `backend_selection` /
`subagent_selection` / `teammate_selection` / `backends_count`,
but `draw_backend_switch_popup` projected only the Active
section as a flat list. Rewrote it to render three labelled
sections with a `▸` marker on the Tab-active one, "Disabled (use
active backend)" leader in the override sections, and
`[Active]` / `[Selected]` suffixes. New `BackendSwitchIntent::Clear`
lets Del/Backspace reset an override to Disabled. Section-aware
Enter dispatches `switch_backend` / `AgentBackendState::set` per
active section.

Header `sub:` / `team:` / `Reqs:` plumbed: `GpuApp` now captures
`ProxyServer::subagent_backend()` /
`ProxyServer::teammate_backend()` /
`ProxyServer::observability()`. `draw_header` resolves the
`AgentBackendState::get()` ids back to `display_name` via the
live config and sums `MetricsSnapshot::per_backend[*].total` for
the Reqs counter. New `UserEvent::TickRedraw` fires every second
through an abortable loop so Uptime / Reqs refresh even when the
PTY is silent.

`restart_pty()` drops the current `ChildPty` (master close →
SIGHUP to the child → reader thread exits on EOF), rebuilds the
emulator at the current `grid_size`, resets `ScrollState` /
selection / momentum, re-spawns with the captured
`spawn_command` / `args` / `env`. Wired to Cmd+R. Plus 1px dim-
grey rect separators below the header and above the footer.

Smoke test: claude welcome screen renders cleanly. Header values
update live. Cmd+R cycles claude. But the title still showed
"Claude CodClaude Code v2.1.152", and the prompt cursor was
invisible.

**Wave 2 — diagnostic infrastructure first.** Two zero-cost-when-
unused additions: `ANYCLAUDE_DEBUG_PTY=/tmp/pty.bin` tees raw PTY
bytes to a file from the reader thread; `Cmd+Shift+D` dumps
grid_size, cursor row/col/visible/style, visible rows + non-zero
flags to stderr. Both were a "should have shipped this on day
one" realisation — without them the FIX-4 PTY-trace process
required `script -q -F` and external ceremony.

**FIX-8 — OSC sliced at UTF-8 continuation byte 0x9C.** The
`Cmd+Shift+D` dump on the still-broken title showed:

```
row[00]: " Claude CodClaude█Code v2.1.152                             "
title: ""
```

The empty `title:""` was the tell. Claude's `ESC ] 0 ; ✳ Claude
Code BEL` should have set the window title. The hex dump (from
the new env tee) showed the OSC payload bytes
`e2 9c b3 20 43 6c 61 75 64 65 20 43 6f 64 65 07`
followed by the rest of the welcome screen.

Read the OSC string handler. It matched three terminators:
`0x07` (BEL), `0x1B` (possible ESC \\), and `0x9C` (8-bit C1
String Terminator). The middle byte of `✳` (U+2733) is `0x9C`.
**The parser was treating the middle of a UTF-8 multibyte
sequence as a string terminator.**

What happened: parser sees `1b 5d` (OSC start), buffers `30 3b`
(`"0;"`), buffers `e2`, then sees `9c` and triggers the 8-bit ST
branch. `dispatch_osc` fires on the partial `[0x30, 0x3b, 0xe2]`
buffer — `std::str::from_utf8([0xe2])` returns Err, `SetTitle`
silently dropped. State back to ground. Next byte `b3` is a
continuation byte at ground state — invalid UTF-8 lead, ignored.
The remaining ` Claude Code` (12 chars) is just plain text,
printed into row 0 cells 0-11. Then BEL (no-op in ground state),
then the actual rendering sequences. CHA 12 + BOLD "Claude" at
cells 11-16, CHA 19 + BOLD "Code" at cells 18-21, CHA 24 +
"v2.1.152" at cells 23-30 — these all overrode parts of the
spilled OSC payload, producing the visible "Claude CodClaude
Code v2.1.152" duplicate. The `█` at col 17 was an alpaca char
(printed at the wrong grid position because cells 1-10 were
already occupied by OSC spill) that survived all the CHA
overrides.

The fix is removing one branch from `osc_string`. Eight bytes of
code. The comment took longer:

```rust
// NOTE: 0x9C (8-bit C1 ST) is intentionally NOT a terminator
// here. In UTF-8 mode (the universal default that Claude Code
// and every modern shell use), 0x9C appears as a CONTINUATION
// byte inside multibyte sequences — e.g. as the middle byte of
// ✳ (U+2733, encoded `e2 9c b3`). Treating it as ST would
// slice the payload mid-character, drop the trailing byte at
// ground state as an invalid UTF-8 lead, then print the
// remaining payload bytes as plain characters in the grid.
// OSC senders that genuinely need ST use the 7-bit form `ESC \`.
```

The lesson generalises: **8-bit C1 control codes (0x80-0x9F)
cannot be honoured in a UTF-8 terminal.** Every byte in that
range can appear as a UTF-8 continuation byte. The 7-bit ESC-
prefixed forms remain valid.

**FIX-9 — INVERSE on default fg/bg collapsed to invisible.**
With the title fixed, the next pass revealed the prompt cursor
was missing. Claude's faux-cursor is the standard ink idiom:
`CSI 7 m SP CSI 27 m` — an inverse-video space. The trace
confirmed it was being sent.

Read `populate_panel`'s inverse handling:

```rust
let inverse = cell.flags.contains(CellFlags::INVERSE);
let (fg_eff, bg_eff) = if inverse {
    (cell.bg, cell.fg)
} else {
    (cell.fg, cell.bg)
};

if bg_eff != TermColor::Default {
    rects.push(RectInstance { ... bg_eff color ... });
}
let is_blank = cell.c == ' ' || cell.c == '\0';
if is_blank && fg_eff == TermColor::Default && !has_decoration {
    continue;
}
```

For a default-coloured inverse space: `cell.bg = Default`,
`cell.fg = Default`. The swap produces `(Default, Default)`.
`bg_eff != Default` is false → no bg rect pushed. `is_blank` is
true, `fg_eff == Default` is true → `continue`. **Zero pixels
rendered.** The "block cursor" was a swap of nothing for
nothing.

Fix is to resolve `TermColor::Default` to concrete RGBA before
the swap. The bg side stays `Option<[f32; 4]>` so the non-
inverse default path can still skip the rect push and let the
window clear-color show through; the inverse path falls back to
a new `DEFAULT_BG = [0.04, 0.04, 0.06, 1.0]` (matching the
renderer surface clear color) when no explicit bg was set:

```rust
let fg_concrete = if cell.fg == TermColor::Default {
    DEFAULT_FG
} else {
    cell.fg.to_rgba(palette)
};
let bg_explicit: Option<[f32; 4]> = if cell.bg == TermColor::Default {
    None
} else {
    Some(cell.bg.to_rgba(palette))
};
let (fg_eff_rgba, bg_eff_rgba) = if inverse {
    (bg_explicit.unwrap_or(DEFAULT_BG), Some(fg_concrete))
} else {
    (fg_concrete, bg_explicit)
};
```

The blank-glyph short-circuit also gains an `!inverse &&
bg_eff_rgba.is_none()` clause so an inverse blank doesn't get
skipped after the rect push.

The user confirmed: cursor visible. Phase 5 closed.

**Workflow lessons added to memory.** Two new lessons join
`feedback_capture_pty_bytes_for_render_bugs`: when adding a new
state-machine handler that recognises C1 control codes, audit
every occurrence against UTF-8 reality; when implementing
INVERSE / xterm reverse-video, do the Default→concrete
resolution before the swap, not after.

## 29. Branch state at end of Phase 5

~120 commits on `feat/gpu-terminal`. Five crates (mvi preserved
per user mandate). GPU UI runs Claude Code end-to-end with no
known visible bugs. The `--gpu` flag is still opt-in pending
cutover.

Next milestone: cutover commit deletes the legacy ratatui code
paths (`src/ui/{render,terminal,header,footer,layout,...}.rs`,
`src/pty/`, etc.), removes the `--gpu` flag, routes `main.rs`
directly to `ui::gpu::run`, drops `ratatui` / `crossterm` /
`alacritty_terminal` / `arboard` / `term_input` from
`Cargo.toml`. Explicitly preserves the `mvi` crate and the
`src/ui/{backend_switch,history,settings,pty}/` MVI stores per
user mandate.

## 30. Phase 5 cutover — deleting the legacy path

Four commits, all green. The plan from §29 landed exactly as
described.

**Commit 1 (`13d50f2`) — main.rs to GPU default.** Drop the
`--gpu` flag, the crossterm raw-mode handling, and the legacy /
GPU branch. `main.rs` now loads config, validates `--backend`,
and hands off to `ui::gpu::run`. The crossterm + `IsTerminal`
probe go away — winit owns keyboard input. Kept first because
nothing else depends on it; `cargo check --workspace` still
passes with the legacy `ui::run` simply becoming unreachable
from the binary.

**Commit 2 (`08faeb7`) — deleting the legacy code.** One
sweeping deletion of every file the GPU UI doesn't need:

```
src/ui/app.rs            src/ui/render.rs
src/ui/events.rs         src/ui/runtime.rs
src/ui/footer.rs         src/ui/selection.rs
src/ui/header.rs         src/ui/terminal.rs
src/ui/input.rs          src/ui/terminal_guard.rs
src/ui/layout.rs         src/ui/theme.rs
src/ui/components/       src/ui/backend_switch/dialog.rs
src/ui/history/dialog.rs

src/pty/                 src/clipboard.rs
src/ipc/                 src/shutdown.rs
src/error.rs
```

`src/lib.rs`, `src/ui/mod.rs`, and the surviving MVI store
`mod.rs` files lose their `pub mod` / `pub use` references to
the deleted modules. Net: −13K LoC of legacy. `cargo check
--workspace` stays green because the deletions are internally
consistent — every deleted module was only consumed by other
deleted modules or by the `tests/` directory.

**Commit 3 (`f9d07d5`) — pruning tests.** Ten test files
deleted: `app_lifecycle`, `app_startup`, `args_pipeline`,
`clipboard`, `error_registry`, `ipc`, `pty_passthrough`,
`restart_claude`, `test_shutdown`, `word_selection`. Each
targeted a now-removed module. The shared `tests/common/mod.rs`
loses its App / PTY helpers (`make_app`, `make_app_with_pty`,
`SpyWriter`, `MockMasterPty`, the `SharedEmulator` alias);
keeps the network / config utilities (`free_port`,
`temp_config`, `wait_for_server`, `mock_backend`) that proxy /
sse / pipeline tests still rely on. `cargo test --workspace`:
all passing — including every MVI store test (`history_actor`,
`history_state`, `pty_actor`, `pty_state`, `settings_actor`),
every proxy / backend / config / shim / metrics test, and the
`term_core` integration suite (56 tests).

**Commit 4 (`f0693bd`) — dropping legacy dependencies.**
`Cargo.toml` loses `ratatui`, `crossterm`, `signal-hook`,
`alacritty_terminal`, `arboard`, `term_input`. The workspace
members list loses `crates/term_input`; the directory itself is
gone. `portable-pty` stays — the GPU UI's `ChildPty` spawns
through it. `Cargo.lock` will regenerate on next build and drop
the transitive closure of the removed crates.

The user's smoke test on the resulting binary: "работает." That's
the end of Phase 5.

**Total deletion**: ~13K LoC of legacy ratatui code, 10 test
files, 6 dependencies, one workspace crate. The remaining
codebase is the GPU UI, the four custom crates that back it
(`term_core`, `term_gpu`, `term_layout`, `term_clipboard`), the
`mvi` framework, the proxy + config + metrics + shim
infrastructure, and the MVI store backbone (preserved per user
mandate even where the GPU UI doesn't consume it yet).

## 31. Branch state at end of Phase 5 cutover

~125 commits on `feat/gpu-terminal`. Five workspace crates:
`mvi`, `term_core`, `term_gpu`, `term_layout`, `term_clipboard`
(was six — `term_input` removed). The binary entry is
`./target/release/anyclaude` (no flags, no opt-in); it runs
Claude Code end-to-end through the proxy on the GPU UI.

What's next: Phase 6 polish items from the spec that didn't
ship in the closing pass — font fallback configuration is the
main one. No more cutover, no more legacy. The next
deliverable is whatever the user prioritises against an open
slate.

## 32. Phase 5 module decomposition — splitting the 2.4K-LoC app.rs

User flagged a SOLID violation: "ты не следовал правилам
проекта, когда писал код gpu". Read the project's feedback
memory, found my own note inside `gpu-terminal-remaining-bugs.md`
saying

> "gpu/app.rs is ~1900 LoC. Approaching the size where
>  extraction into submodules ... would help."

— and over the following Phase 5 work I had added ~500 more LoC
(closing pass + cutover) directly to that file. The struct
mixed PTY lifecycle, winit `ApplicationHandler`, chrome
rendering, popup rendering, bootstrap setup, diagnostic dump,
clipboard, and selection — every concern lived in one impl.

Five atomic commits split it. Each kept `cargo check
--workspace` green; per-commit changes ordered so no
intermediate state needed `#[allow(dead_code)]` or other linter
band-aids.

**REFAC-1 (`c426c06`) — `chrome.rs` (234 LoC)**. `draw_header`,
`draw_footer`, and all chrome constants (`HEADER_HEIGHT_LOGICAL`,
`FOOTER_HEIGHT_LOGICAL`, `CHROME_*`, `SESSION_COPY_FLASH`,
`APP_VERSION`, `FOOTER_HINTS`, `CHROME_FONT_SIZE`).
`CHROME_TEXT_COLOR` and `CHROME_FLASH_COLOR` are `pub(super)` so
popup code in the next commit can use them too without
duplicating the values.

**REFAC-2 (`25ad48a`) — `popup.rs` (979 LoC)**. Every popup
helper plus `POPUP_*` constants and `DEFAULT_FG_FOR_POPUP_SELECTED`.
Three `pub(super)` entry points: `draw_backend_switch_popup`,
`draw_history_popup`, `draw_settings_popup`. Internal helpers
(`push_section_header`, `push_backend_item`,
`push_override_section_rows`, `draw_string_list_popup`,
`draw_popup_chrome`) stay private. `override_selection_to_backend_id`
exposed because the keyboard handler in `app.rs` calls it on
Enter to map selection index → backend id.

**REFAC-3 (`5181d74`) — `diagnostic.rs` (57 LoC)**. The
Cmd+Shift+D snapshot dump as a free function taking borrowed
pieces of `GpuApp` state (`grid_size`, `scroll.offset_y`,
`scroll.max_offset()`, `Option<&RenderSnapshot>`) instead of
`&self`. The keyboard handler builds the snapshot once and
hands the slices in. No more `self.dump_diagnostic_snapshot()`
method; the App stops being responsible for its own dumper.

**REFAC-4 (`a146a72`) — `bootstrap.rs` (172 LoC)**. The
`run()` entry point with everything it owns: config / settings
/ debug logger / tokio runtime / proxy server bind / teammate
shim / spawn-param assembly / event loop construction / hand-
off to `GpuApp::new`. `gpu/mod.rs` re-exports `run` from
`bootstrap` instead of from `app`. `GpuApp::new` and
`enum UserEvent` become `pub(super)` so bootstrap can build
them. `app.rs` drops eight unused imports that came along with
`run` (`EventLoop`, `build_spawn_params`, `ConfigStore`,
`DebugLogLevel`, `DebugLogger`, `init_global_logger`,
`ProxyServer`, `TeammateShim`).

**REFAC-5 (`da1090f`) — decomposing `draw_backend_switch_popup`**.
The popup function itself was still ~340 LoC of inline rendering:
width measurement, height math, popup chrome, title push, three
section bodies (where Subagent and Teammate were copy-pasted),
and the footer hint. Replaced with a sequence of purpose-named
helpers:

- `compute_backend_switch_popup_width` — measurement only.
- `compute_backend_switch_popup_height` — row math only.
- `draw_popup_title` — title push.
- `draw_popup_footer_hint` — footer push.
- `draw_active_section` — header + sep + items loop with `[Active]`.
- `draw_override_section` — header + sep + Disabled leader + items
  loop with `[Selected]`. Replaces both the previous
  `push_override_section_rows` (just the items) AND the inline
  header+separator code in the two override branches. The
  Subagent/Teammate duplication is gone.
- `push_section_header_with_separator` — shared header+separator
  rendering used by both section helpers.

`draw_backend_switch_popup` is now ~140 LoC and reads as the
layout sequence: extract state → measure → place → draw chrome →
for each of (title, active section, subagent section, teammate
section, footer hint), draw it.

**The RenderCtx that didn't ship.** Originally REFAC-5 was going
to introduce a `RenderCtx<'a>` struct grouping `(font_system,
swash_cache, atlas, ui_shape_cache, glyphs, rects, sf)` — the
parameter chain that travels through every chrome and popup
function — so the `#[allow(clippy::too_many_arguments)]` markers
could be dropped. User asked: "зачем нужен RenderCtx". The
honest answer was: there's no consumer beyond the linter
complaint. One callsite (`redraw`) constructs the struct; the
helpers are already isolated inside their modules; lifetime
ceremony and overlay-layer reborrow would add more pain than
the marker. The marker is a finger pointing at the function;
the fix belongs at the function, not at a wrapper that hides
it. So REFAC-5 became "decompose the function itself" — and
the helpers it spawned naturally have shorter parameter
chains, with the remaining `#[allow]` on the orchestrator and
on `push_backend_item` honestly noting that the rendering
domain genuinely couples those parameters.

**Result**: `src/ui/gpu/app.rs` 2400 → 1470 LoC. Each
submodule has a single responsibility — chrome / popup /
diagnostic / bootstrap are independently maintainable. The
remaining `app.rs` is the `GpuApp` struct + `winit::
ApplicationHandler` impl + PTY lifecycle + scroll / selection
/ input handlers.

The work was triggered by user feedback, not by me proactively
splitting the file. Lesson saved to feedback memory: when my
own architecture notes flag a file's size as a problem, the
split is overdue — don't add to it on the next feature.

## 33. Branch state at end of REFAC-5

~130 commits on `feat/gpu-terminal`. Five workspace crates
(unchanged from §31). The binary entry is unchanged:
`./target/release/anyclaude`. Two user smoke tests this
session: one after cutover ("работает"), one after refactor
("работает"). No visual regression — refactor was pure
restructure.

The branch is ready to merge or rename to default. Phase 6
polish items remain (font fallback configuration), but nothing
in the spec requires more architectural restructuring before
they ship.

## 34. The term_ui pivot — dropping MVI for one AppState + a retained engine (2026-05-29)

The GPU UI worked, but the user pressed on quality: "говнокод",
`app.rs` still huge, "не ясно, используется ли mvi как должен". The
honest audit: MVI was used for the 3 popups and **bypassed for the
whole terminal surface** (~27 raw `self.<field> =` mutations for
scroll/selection/cursor/follow), plus a fully-written-but-unused
`PtyActor`. Split-brain. The user asked **"может нафиг этот mvi?"** and,
valuing one consistent model over a half-applied framework, chose to
drop it.

The reframe that settled it: MVI's unique benefits (intent log, replay,
strict unidirectional types) are unused in a single-user terminal; what
it actually bought here (testable transitions, view/logic separation)
survives without the `Store/Actor/Intent` ceremony. Decision: **no MVI.
One plain `AppState` = single source of truth; a retained + reactive,
GPUI-style-authored kit (`term_ui`) renders `view(&AppState)`.** The
state model gets simpler; the complexity moves into the view engine,
where the five demanded capabilities (animation, focus, text fields,
long-list virtualization, retained scroll) actually live. The one
principled exception to "single source": the VT emulator's
grid/cursor/scrollback stay in `term_core` (bucket 3-T) — they can't be
a plain `Clone+PartialEq+Default` struct and have a single writer (PTY
bytes). "Single source" was redefined as *one writer per fact*, not
*one struct*.

**The design doc** (`docs/design/term-ui-design.md`, ratified) was
produced by a four-phase workflow: parallel research (Warp's `warpui` +
Zed's GPUI + Linebender's Xilem + our own gpu code), a synthesis pass, a
five-lens adversarial review, and an assembly that resolved the findings
inline. Fifteen checkable invariants R1–R15 are its spine.

**Phase A** (engine core: generational arena, `Element` trait, positional
splice, Flex-lite layout, the R4 "rebuild == incremental" property gate)
was **built by a workflow** and then **hand-validated** — not "cargo
test passed" but a semantic audit against R1–R15 plus *testing the
tests*: stubbing `reconcile` to a no-op reddened 6 R4 cases, proving the
gate is not a tautology. (The implementer agent's socket dropped
mid-run; the fix agent still completed the blockers — a reminder to
trust nothing and audit the working tree, not the agents' reports.)

**Phase B** (the coordinator: one `AppState`, the two-phase `event → Msg
→ apply → reconcile` frame, a single dirty signal, incremental reconcile,
`frame_now` threading, a `next_wake` ticker) the user asked me to author
myself. The decisive bug was found not by tests but by *running it*:
holding a key froze the timer — key-repeat event churn starved
`StartCause::ResumeTimeReached` because `about_to_wait` recomputed
`now + TICK` each call and the deadline kept sliding forward. Fix: poll
an absolute `next_tick` in `about_to_wait` (which runs after every event
batch). The lesson generalises — caret blink and animations would starve
identically; tickers must poll absolute deadlines.

**Phase C** (port the real header/footer to declarative term_ui views) I
authored myself too. The interesting outcome was the *layering*, not the
rendering. A new `uikit` crate holds the **generic, domain-agnostic**
bars — `Segment {text, color}` + `header_bar`/`footer_bar` over term_ui —
and the "backend:/sub:/Reqs:/Session:" vocabulary went into an
`ui::chrome_labels` presenter in anyclaude `src/`, *not* the kit (a
reusable kit must not spell its app's words). Two YAGNI wins fell out:
the design sketch's planned `RichRow` widget was never needed — plain
`Stack`/`Text`/`Spacer` compose the row, the 1px fence is just a
`Block`-over-`Spacer` pinned `Fixed(1)` + `Stretch`, and the footer
right-aligns its version with `Spacer::fill()`. It is proven in
`examples/chrome_preview.rs` — the real chrome through a real
`GpuRenderer` on the Phase B coordinator pattern — and was user-verified.
The session-click hitbox and scroll/momentum were *deferred*: they need
R7 event routing and the real coordinator that replaces `GpuApp`, which a
fake-data preview can't stand in for.

**Phase D** killed MVI for the three popups (backend-switch, history,
settings). Each MVI `Actor::handle_intent` became an inherent
`apply(&mut self, intent)` on the state enum — the same reduce logic,
mutated in place — the `mvi::{State,Intent,Actor}` impls dropped, the
Actors deleted, and `GpuApp` migrated from three `Store<…>` fields to
three plain-state fields (`dispatch` → `apply`). The user chose the
*cutover* path (touch the live `GpuApp`, delete the actors now) over the
additive one, so this was the first phase to modify the live app rather
than build beside it — done as a **state-only** cutover (the popup
*rendering* stays the existing immediate draw; its term_ui-view-ification
defers to the coordinator in E, the same deferral C made for its
hitbox/scroll). Two findings worth keeping: every popup is
navigation-plus-toggle with **no text input**, so the planned
TextField/caret work was pure YAGNI and skipped; and the settings
`RequestClose` dirty-discard flow turned out to be unit-tested but never
wired to the live Escape — a latent feature to resolve when the
coordinator owns Esc routing. `mvi` now has exactly one consumer left,
the dead `PtyActor`, so the crate's deletion is a clean Phase F. All
~600 project tests stayed green; verified live.

Status at handoff: Phase A + B + C (chrome views) + D (popups off MVI)
done and verified; Phase E (dissolve `GpuApp` into the real coordinator,
land all the deferred term_ui views) is next, then F deletes `mvi`.
