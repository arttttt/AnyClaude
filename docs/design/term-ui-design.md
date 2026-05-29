# term_ui — Design Doc (RATIFIED 2026-05-29)

> Status: **RATIFIED (2026-05-29).** The invariants R1–R15 (§1) are binding; every term_ui commit + review checks against them. **Implementation status:** Phase A ✅ (`159e3aa` — engine core + the R4 property gate) · Phase B ✅ (`816dba4`/`7d3c62f`/`3d4af45` + ticker fix `f7d9dfe` — the coordinator pattern, proven in `crates/term_ui/examples/coordinator.rs`) · Phase C ✅ chrome views (`135fcb2`/`2e64e82`/`360dafa`/`1dcf6f2` — a new domain-agnostic `uikit` crate: `Segment` + `header_bar`/`footer_bar` over term_ui, fed by the `ui::chrome_labels` presenter in anyclaude `src/`; proven in `examples/chrome_preview.rs`, user-verified. The original §15 sketch's RichRow widget proved unnecessary — plain `Stack`/`Text`/`Spacer`/`Block` sufficed — and the session-click hitbox + scroll/momentum are deferred to the real coordinator that replaces `GpuApp`) · **Phase D next** (port the 3 popups, delete their actors — see §15). All Rust is **illustrative sketch only** — it shows shape and intent, not final API. Warp citations are `path:line` under `/Users/artem/Projects/warp`. term_gpu/term_core citations are real signatures verified in `/Users/artem/Projects/ClaudeWrapper/crates/`. Adversarial review findings (four lenses) are resolved inline; each resolution is tagged **[Resolved: …]** or **[Rejected: …]**.

---

## 1. Invariants / Design Rules (the SPINE)

These are the objective, checkable invariants. Every later section justifies itself against them; every future code review checks against them.

- **R1 — No MVI, no Store/Actor/Intent.** The `mvi` crate, the three popup `Store<…Actor>` fields, and the dead `PtyActor`/`PtyIntent`/`PtyLifecycleState`/`PtySideEffect` are deleted (with user sign-off, §16-Q1). No `dispatch`, no `reduce`, no `Effect` queue survives. *Checkable: `grep -r "mvi\|Store<\|::dispatch\|Intent\b" src/ crates/term_ui/` returns nothing.*
- **R2 — One AppState, plain structs.** Exactly one `AppState` value holds ALL authoritative UI-**decision** state. No `Arc<RefCell<…>>` / `Rc<RefCell<…>>` inside `AppState`. *Checkable: `AppState` and its sub-structs contain only owned plain data + `Option`/`Vec`/`HashMap`; no interior-mutability cells.* **[Resolved — single-source lens issue 1: the word "decision" is load-bearing. The VT emulator's grid/cursor/scrollback/protocol-flags are authoritative *content* but are NOT a UI decision; they are external-IO-driven and live in bucket 3-T (see R3 and §3). R2 governs UI-decision state only.]**
- **R3 — Four buckets, no leakage.** Every piece of state belongs to exactly one of: (1) `AppState` = authoritative UI-decision state; (2) retained tree = derived/cache only; (3-S) resources/services = handles to external resources; (3-T) **terminal-protocol state** = authoritative content owned by the emulator service, intentionally outside `AppState` because it is external-IO-driven, single-writer (PTY bytes), and re-derivable only by replaying the byte stream. The classification in §3's table is binding. *Checkable: each new field maps to a bucket in the §3 table or extends it.* **[Resolved — single-source lens issue 1 (blocker): bucket 3 is split into 3-S (handles) and 3-T (terminal-protocol authoritative state) so the emulator is no longer mislabeled a "handle." Its membership and the cross-store dependency are made explicit in §3.]**
- **R4 — Retained tree is authoritative of NOTHING; rebuild is pixel-identical under a fixed frame clock.** Destroying the entire retained tree and rebuilding it from `view(&AppState)` + window size + a **fixed `frame_now: Instant`** + a **fresh/normalized resource-cache state** yields a **pixel-identical** frame. *Checkable: a property test compares the layout-and-paint output of `rebuild_from_scratch` vs `incremental_rebuild` for the same `(AppState, window, frame_now)`, comparing instance **geometry+color+glyph-identity (CacheKey)** while **normalizing away atlas UVs and frame counters** (or by rendering both into a fresh offscreen texture and comparing framebuffers). This is an acceptance gate for §15 Phase A.* **[Resolved — single-source lens issue 6, yagni-nodefer issue 3, rust-feasibility issue 4, dissolution issue 6: R4 now (a) threads a fixed `frame_now` so time-derived blink/anim/flash are deterministic, and (b) defines the comparison to be invariant to service-cache state (atlas `last_used_frame` / `ShapedTextCache` frame counters / shelf-packing UVs are path-dependent and bucket-3-S, so they are excluded from the equality). "Pixel-identical" ≠ "instance-stream-identical"; the gate compares layout+paint geometry/color, not raw UVs.]**
- **R5 — Retained + reactive, GPUI-style authoring.** `view(&AppState, frame_now) -> ViewTree` reads like immediate-mode; a retained element tree + incremental reconcile/layout/paint live under the hood. We re-author the view tree from a changed region; we do NOT rebuild the whole tree every frame, and we do NOT keep a virtual DOM diff at the leaf — diff is structural per-node `rebuild(prev, self)` (Xilem model). Incremental relayout applies to **chrome subtrees** (header/footer/popups); the terminal grid re-emits its full visible window each frame (see R5-note and §5/§10). **[Resolved — rust-feasibility issue 3 (major): the original "VT parser dirty-line info" does not exist (`TerminalEmulator` exposes only `process/resize/snapshot/take_responses` + mode getters; `RenderSnapshot{rows,visible_rows,cursor,title,cwd}` is a full clone, no per-line dirtiness). The incremental-relayout payoff is scoped to chrome, where `rebuild(prev,self)` field-diff genuinely skips work. The grid was never the bottleneck — it is bottom-anchored and cheap via the shape caches.]**
- **R6 — One mutation path → one reconcile.** The only way the UI changes is: `event → mutate AppState (and/or drive a service) → reconcile(view(&AppState, frame_now))`. No scattered `request_redraw`; no widget mutates AppState during paint. There is exactly one place that observes "AppState changed" and triggers reconcile (a single dirty signal).
- **R7 — Reconcile/event-phase separation.** Events never hold a `&` borrow of the retained tree while mutating `AppState`. The frame is phases: **event phase** (read tree hit-geometry → produce `Msg`), **apply phase** (`apply(&mut AppState, Msg)`), **reconcile phase** (`&AppState` borrowed immutably to rebuild the tree). The phases never overlap in a borrow. Event routing is a **non-recursive linear scan** over derived geometry, not a tree walk (see §6/§14). **[Resolved — rust-feasibility issue 1: `View::event` is removed from the trait; routing is a free fn over the materialized `hitboxes`/`focus_order` vectors. This makes the phase boundary a type-level fact and avoids the `&mut AppState`+`&mut RetainedTree` recursive-reborrow trap.]**
- **R8 — Two identities: stable `WidgetId` (AppState-facing) vs arena `NodeId` (bucket-2 only).** Authoritative AppState that is keyed by widget (`focus`, `fields`, `animations`, `hover`) is keyed by a **stable id-path `WidgetId`** (Xilem `ViewId`-style; survives full teardown+rebuild and reordering). The retained tree's internal slots use a **generational `NodeId`** (arena index). The arena maps `WidgetId ↔ NodeId` per frame. We do NOT rely on `Arc` object identity (Warp's `MouseStateHandle` smell — `hoverable.rs:159` — is explicitly rejected). `RefCell`/`Rc` is permitted ONLY where §14 justifies it in writing (none required). **[Resolved — rust-feasibility issue 7 (and §16-Q3, now closed): keying bucket-1 state by an arena slot would orphan a field's caret/value on any structural reorder (ABA bug). Authoritative identity MUST be the stable id-path; the arena slot is internal-only. The draft's interchangeable use of "WidgetId/arena slot" was the bug.]**
- **R9 — term_ui layers on term_gpu; never duplicates it.** term_ui consumes `RectInstance`/`GlyphInstance`/`ShadowInstance`/`RenderLayer`, `GpuRenderer::render`, `GlyphAtlas`, `FontSystem`/`TextShapeCache`/`SwashCache`, `populate_panel`/`build_cursor_rect`/`measure_cell_metrics`, `ScrollState`+momentum, `Selection`. term_ui re-implements none of these. The ONLY migration is `label.rs` (`push_label`/`measure_label_width` + a new `caret_x`/`byte_at_x` pair built on the same `ShapedLine`) — it has zero VT coupling and is UI text. *Checkable against term_gpu inventory §12.*
- **R10 — GpuApp dissolves into 3 things.** (a) a thin coordinator (resources + `AppState`), (b) `view(&AppState, frame_now)`, (c) pure tested logic functions. "Thin coordinator" means **handlers translate events to `Msg` and call `apply`+`reconcile`; the *ordered fan-out logic* (e.g. resize→renderer.resize→invalidate cell_metrics→recompute grid) lives in `apply` and pure fns, not in the handler body.** *Checkable: the coordinator's `window_event`/`user_event` bodies contain no branching domain logic beyond event→Msg mapping.* **[Resolved — dissolution issue 5: "no logic in handlers" was too absolute; resize/scale-factor branches do real ordered invalidation. Clarified that the logic moves into `apply`, not that it vanishes.]**
- **R11 — Five capabilities are designed now, no deferral.** animations/transitions; focus + full keyboard traversal; text fields (caret + in-field selection); incremental relayout incl. long-list virtualization; retained scroll-state across frames. Each has a fully-specified home in §8/§9/§10 NOW (including the GPU mechanism for animation, the caret↔pixel mapping for text fields, the next-wake scheduling for blink, and the focus transitions on popup open/close). Widget *surface area* grows with consumers (YAGNI), but the *engine capability* is not phased out.
- **R12 — Derived state is computed, never stored authoritatively.** Caret blink phase, hover-highlight, virtualization window, measured sizes, shaped glyphs, header label strings, scroll-bound clamps, eased anim value, session-copied flash: all are pure functions of `AppState`(+window)(+`frame_now`). They may be *cached* in bucket 2 but are authoritative of nothing (R4).
- **R13 — English-only, atomic commits, SOLID/DRY/KISS/YAGNI.** Every subsystem is justified by one of the 5 capabilities (R11) or a current consumer: header, footer, 3 popups (backend-switch/history/settings), terminal panel. No abstraction precedes its consumer. No-phase-deferral overrides YAGNI only for what R11 requires.
- **R14 — Single overlay, single popup.** `GpuRenderer::render(base, overlay: Option<RenderLayer>, scroll_offset_y)` accepts exactly one overlay (`renderer.rs:177`). **At most one popup is open at a time** — `close_all_popups()`/`toggle_*()` enforce mutual exclusion (`app.rs:178`, comment "At most one is `Visible` at a time"), so the open (or closing, §9) popup occupies the single overlay layer. term_ui does **not** support stacked popups or a popup-over-popup compositor. **[Resolved — yagni-nodefer issue 1 (major): the "merge any popup-over-popup into one overlay" machinery had no consumer. Reduced to the true invariant: one popup, one overlay. If stacking is ever needed, it is added with its consumer.]**
- **R15 — Per-frame cache lifecycle is term_ui's responsibility.** `GpuRenderer::render` calls `atlas.upload`+`atlas.end_frame` only (`renderer.rs:251`). term_ui MUST call `TextShapeCache::end_frame()` (`text.rs:379`) on each owned cache once per frame, or caches grow unbounded.

---

## 2. Architecture overview

term_ui is a **retained + reactive** UI engine authored GPUI-style, sitting between `AppState` and `term_gpu`. Authoring feels immediate-mode (`view(&AppState, frame_now)` returns a fresh declarative tree); execution is retained (a persistent element/layout tree is incrementally reconciled, not rebuilt wholesale).

Three nested loops, outermost to innermost:

1. **Reactive loop (per input/timer):** an OS/PTY/timer event becomes a `Msg`; `apply(&mut AppState, Msg)` is the single authoritative mutation (it may also drive a service: PTY write, renderer resize); a dirty signal fires.
2. **Reconcile loop (per dirty AppState):** `view(&AppState, frame_now)` produces a transient view tree; the engine diffs it against the retained tree via per-node `rebuild(prev, self)` (Xilem); only changed **chrome** subtrees re-layout (incremental), the terminal grid re-emits its visible window.
3. **Paint loop (per frame):** index-based passes over the arena walk `measure → place → paint`, emitting `Vec<RectInstance>`/`Vec<GlyphInstance>`/`Vec<ShadowInstance>`; term_ui slices them into a base `RenderLayer` (+ optional overlay) and calls `GpuRenderer::render`.

### Prose data-flow diagram

```
       ┌──────────── BUCKET 3-S: RESOURCES / SERVICES (handles) ───────────┐  ┌── BUCKET 3-T: TERMINAL PROTOCOL STATE ──┐
       │ GpuRenderer(owns GlyphAtlas) · FontSystem · SwashCache ·          │  │ TerminalEmulator owns:                   │
       │ TextShapeCache×2 · ChildPty · Clipboard · BackendState/Agent     │  │  grid · cursor · scrollback ·            │
       │ handles · ObservabilityHub · ClaudeSettingsManager · timers      │  │  mouse_mode · bracketed_paste ·          │
       └───────────────────────────────────────────────────────────────────┘  │  cursor_keys_app · focus_reporting ·     │
                  ▲ &mut for measure/paint        ▲ commit-through-handle      │  title · cwd                             │
                  │                               │                            │ authoritative CONTENT, NOT a UI decision │
  winit/PTY/timer ─event─► [EVENT PHASE]          │                            │ single-writer = PTY bytes (R3 3-T)       │
                          linear hit-scan over     │                           └──────────────────────────────────────────┘
                          tree GEOMETRY only ──► Msg                            │ snapshot() each frame (full clone)
                              │                    │                            ▼ read-only for paint
                              ▼                    │              ScrollState.total_size_px is RE-DERIVED from
                  apply(&mut AppState, Msg)  ◄─ ONE mutation path (R6); may drive a service
                              │  (sets ONE dirty signal)
                              ▼
   ┌── AppState (BUCKET 1, authoritative UI-DECISION state) ──────────────────────────┐
   │ scroll:ScrollState · focus:WidgetId · selection · fields(caret+sel+epoch) ·       │
   │ animations(targets/timers) · hover · popups(incl. Closing phase) · session_flash  │
   └────────────────────────────────────────────────────────────────────────────────┘
                              │ &AppState (immutable) + frame_now (fixed for this frame)
                              ▼
                 view(&AppState, frame_now) ──► transient ViewTree
                              │ rebuild(prev,self) per node (Xilem); chrome incremental, grid full-emit
              ┌── RETAINED TREE (BUCKET 2, derived/cache; authoritative of NOTHING, R4) ──┐
              │ arena Vec<Node> · placed boxes · measured sizes · virtualization window ·  │
              │ shaped-glyph cache · hitboxes:Vec<(Bounds,WidgetId)> · focus_order:Vec<…>  │
              └────────────────────────────────────────────────────────────────────────────┘
                              │ index-based measure→place→paint passes
                              ▼
       Vec<RectInstance> · Vec<GlyphInstance> · Vec<ShadowInstance>
        (overlay assembler MULTIPLIES eased alpha into every color[3] — see §9)
                              │ slice into RenderLayer { shadows, rects, glyphs }
                              ▼
   GpuRenderer::render(base, overlay: Option<RenderLayer>, scroll_offset_y = 0.0)  ── renderer.rs:177
        (terminal scroll is PRE-BAKED into instance positions by populate_panel; the
         scroll_offset_y uniform stays 0.0 and is NOT used by term_ui — see §10)
                              ▼
                        pixels on screen
```

**[Resolved — capabilities lens issue 4 & single-source notes: the diagram previously showed `scroll_offset_y` as the live scroll path. Verified that all three shaders subtract a single `uniforms.scroll_offset` from ALL instances in BOTH layers (`prim.wgsl:34`, `text.wgsl:40`, `shadow.wgsl:61`), so using it would also scroll chrome/popups — a regression. `populate_panel` bakes the offset into instance positions and the renderer is passed `0.0`. The diagram now reflects that term_ui keeps the bake-into-instances path and the uniform is dead.]**

Key contrasts with the references:
- vs **warpui/GPUI**: we drop the `Entity`/`Handle`/`observe`/`notify`/`subscribe`/autotracking graph entirely — it exists only to coordinate *fragmented* state. One `AppState` collapses all of it to one dirty signal.
- vs **Xilem**: we keep the four-method `View` lifecycle and id-path identity, but **drop per-node event message-routing** (events route by linear hit-scan, §6), `no_std`, the `Context: ViewPathTracker` generic, `xilem_web` multi-backend, and `AnyView`/`SuperElement` upcasting.
- vs **warpui layout**: we adopt the constraints-down/sizes-up/positions-down `Element` contract (`elements/mod.rs:106-179`) and a Flex-lite, doing incremental relayout on **chrome** (Warp re-lays-out the whole tree every frame; `presenter.rs:331-373`).

---

## 3. State doctrine — four buckets

### Definitions (binding)

- **Bucket 1 — `AppState` (authoritative UI-decision state).** All state whose value is a *decision the app made* and must remember: focus target, scroll offsets, selection, text-field values, caret + in-field selection, animation targets/timers, hover target, popup states (incl. a `Closing` phase). One place. Plain structs (R2). The *only* thing events mutate (besides driving services) and the *only* thing that survives a `view()`/reconcile beyond services.
- **Bucket 2 — Retained tree (derived/cache).** Layout boxes, virtualization window, measured sizes, shaped-glyph instances, paint instances, hitbox list, focus-order list, per-node `ViewState`. Pure memoization of `view(AppState, frame_now) + window`. Destroy + rebuild ⇒ pixel-identical (R4).
- **Bucket 3-S — Resources/services (handles).** `GpuRenderer` (owns `GlyphAtlas`), `FontSystem`, `SwashCache`, `TextShapeCache`×2, `ChildPty`, `Clipboard`, `BackendState`/`AgentBackendState` shared handles, `ObservabilityHub`, `ClaudeSettingsManager`, timer `AbortHandle`s, spawn params, diagnostic service. Own *external resources*, never UI decisions.
- **Bucket 3-T — Terminal-protocol state (authoritative content, emulator-owned).** The `TerminalEmulator` owns: VT **grid**, **cursor**, full **scrollback**, and protocol flags **`mouse_mode()`**, **`bracketed_paste()`**, **`cursor_keys_app()`**, **`focus_reporting()`**, **`title()`**, **cwd** (verified `emulator.rs:53-71`). This is authoritative *content*, not a UI decision and not a handle. It is **single-writer** (only PTY bytes via `process()` mutate it) and re-derivable only by replaying the byte stream. It is read each frame via `snapshot()` (a full clone of visible rows) for paint, and its flags are read by the §6 mouse-reporting gate. **[Resolved — single-source lens issue 1 (blocker): the emulator is the single largest authoritative render-affecting store; the draft hid it under "handles." It now has a named, honest home. The single-source proof (below) accounts for it.]**

**Cross-store dependency (made explicit):** `AppState.scroll.total_size_px` is *derived* from the emulator's scrollback length (today `refresh_scroll_geometry`, `app.rs:341-353`). The **single writer of `total_size_px` is `apply`**, which recomputes it from the emulator snapshot on `Msg::PtyBytesArrived` and on resize. AppState holds the authoritative *scroll offset / clamp* (a UI decision); the emulator holds the authoritative *content extent*. There is exactly one writer per fact; the derivation direction is one-way (content → scroll bounds), never the reverse.

### Decision table for borderline cases

| Item | Bucket | Why |
|---|---|---|
| **focus** (which widget has keyboard) | **1 (`AppState.focus: WidgetId`)** | A remembered decision; survives reconcile; one focused widget per window (Warp `window.rs:48`). A field keyed by stable `WidgetId` (R8). |
| **scroll offset** (terminal) | **1 (`AppState`, as `ScrollState`)** | Authoritative pixel offset; persists across frames. term_gpu's `ScrollState{offset_y,total_size_px,visible_px}` reused. `total_size_px` re-derived from 3-T (above). |
| **scroll offset** (history popup) | **1 (`HistoryUi.scroll_offset: usize`)** | **Row-index**, NOT pixels. The existing history popup scrolls by integer row over fixed-height rows (`history/state.rs:17`, `MAX_VISIBLE_ROWS=14`). It is NOT a `ScrollState`. **[Resolved — single-source lens issue 4, capabilities lens issue 7, yagni-nodefer issue 8: the draft wrongly lumped it with the terminal's pixel `ScrollState` and described SumTree pixel-seeks. Reconciled: history stays row-index `usize`; §10's pixel `visible_range`/SumTree apply only to the (deferred) variable-height path.]** |
| **text-field value** | **1** | Authoritative content (controlled component, §8). |
| **caret position + in-field selection** | **1** | Authoritative cursor/selection within a field (Warp's `SelectionModel` is model-resident — `editor/src/selection.rs:25-46`). |
| **caret blink phase** | **2 (derived) / driven by 1** | NOT stored. `AppState` stores `caret_epoch: Instant` (timer, bucket 1); visible on/off = `((frame_now - epoch).as_millis() / 530) % 2 == 0` — pure (R12, §8). |
| **hover target** | **1 (`AppState.hover: Option<WidgetId>`)** | Authoritative (gates highlight + click meaning). Warp scatters it into `Arc<Mutex<MouseState>>` (`hoverable.rs:159`) only because state is fragmented; with one AppState it's one field. |
| **animation target + timer** | **1 (`AppState.animations`)** | Authoritative "where it's going + when it started." Includes popup `Closing` anims (§9). |
| **animation *current* eased value** | **2 (derived)** | `delta = ease((frame_now - start)/duration)`; never stored. |
| **layout boxes / measured sizes** | **2** | Pure memoization; invalidated by reconcile. |
| **shaped glyphs / glyph instances** | **2** | Output of measure/paint over `&AppState`; cached. (Warp's two-frame `LayoutCache`, `text_layout.rs:62-130`.) |
| **terminal virtualization window** | **derived from 3-T** | The VT grid IS the window; `populate_panel` emits the visible rows. Not a generic list. |
| **history popup visible range** | **2 (derived)** | `f(scroll_offset:usize, MAX_VISIBLE_ROWS=14)` — trivial fixed-row slice; no SumTree. |
| **measured item heights** (variable-height list — DEFERRED) | **2 (cache)** | Only if/when a variable-height list consumer appears (§16-Q4). See R4 caveat below. |
| **session-click-zone** (header hit rect) | **2 (derived hit-geometry)** | Per-frame projection of the header layout; lives in the tree's hitbox list, not `AppState`. |
| **settings working copy** (`fields`, `dirty`, `confirm_discard`) | **1 (authoritative editing state)** | The Visible state holds a mutable working COPY of on-disk settings + a stored `dirty` flag + `confirm_discard` (`settings/state.rs:8-14`). `dirty` is **stored** (matches the ported actor's behavior), NOT derived — it is explicitly excluded from R12. On-disk snapshot is the 3-S service value, committed through `ClaudeSettingsManager` on Enter (single-writer-through-handle). **[Resolved — single-source lens issue 5: called out explicitly so a later reviewer doesn't "derive" `dirty` and diverge from ported tests.]** |
| **`BackendState`/`AgentBackendState`** | **3-S (service)** | `Arc<RwLock<…>>` shared with proxy HTTP threads. The override value lives behind the handle (proxy reads it); the popup seeds from `get_active_backend()` at open and commits via `switch_backend()`/`set()` on Enter (verified `app.rs:461-637`). Transient selection index is bucket 1. Single-writer-through-handle. |
| **`cell_metrics`** | **2 (derived cache)** | `measure_cell_metrics(font_system, …, scale_factor)` memoized; invalidated on scale-factor change (`app.rs:1302`). |

**R4 caveat for lazy height caches (binding):** A *fully-measured* fixed/known-height cache is genuinely re-derivable (Warp's own early-return when all rows measured, `viewported_list.rs:656-657`: `approximate_height == exact`). A **lazy** `SumTree` height cache is **path-dependent** — `approximate_height = (total_measured_height / measured_count) * total_items` (`viewported_list.rs:651-678`), and `absolute_pixels_to_scroll_offset` maps through that running average — so destroy+rebuild that measures a *different* subset yields a different scrollbar thumb and a different pixel↔index map. Therefore: **term_ui does NOT use a lazy SumTree for any current consumer.** The history popup (≤ few hundred rows, 14 visible) uses the fixed-row index model (above), for which R4 holds exactly. If a future variable-height consumer needs lazy measurement, R4 is weakened *in writing at that time* to "pixel-identical for fully-measured content; for lazy estimation, identical only after the same rows are measured." **[Resolved — single-source lens issue 3 (major), yagni-nodefer issue 2 (major): the draft claimed the lazy cache was "authoritative of nothing" and re-derivable to an identical result; verified false. Removed the SumTree from the now-built design; documented the R4 weakening contract for the deferred path.]**

### Proof that single-source is preserved

Single-source means: **there is exactly one writable copy of every authoritative fact.**

1. *UI-decision facts.* All in bucket 1 (focus, scroll offset, caret, text value, hover, anim timers, popup phase incl. `Closing`, settings working copy). A tree rebuild reproduces the same pixels (R4).
2. *Terminal content facts.* All in bucket 3-T, owned by the emulator, single-writer = PTY bytes. **The single-source proof no longer claims "no authoritative fact lives outside AppState"** — it claims "no authoritative fact has two writers." The emulator's grid/flags have exactly one writer (the byte stream); `AppState.scroll.total_size_px` is a one-way derivation with `apply` as its sole writer.
3. *No duplication within bucket 1.* Derived facts (blink phase, eased anim value, history visible range, label strings, clamped scroll bounds, session flash) are computed by pure functions (R12), never stored as a second copy. Exception explicitly recorded: settings `dirty` is stored to match ported behavior.
4. *Reconcile cannot create a divergent source.* `view(&AppState, frame_now)` borrows `&AppState` immutably (R7); it cannot fork a writable copy. The tree it builds is bucket 2.

---

## 4. View / Element model + reconciliation

### The View abstraction (Xilem four-method lifecycle, simplified to one backend, no per-node events)

```rust
// ILLUSTRATIVE SKETCH — not final API.

/// Stable id-path identity (Xilem ViewId). Survives full teardown+rebuild
/// and reordering. AppState (focus/fields/animations/hover) is keyed by this.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WidgetId(u64);

/// Arena slot identity — bucket-2 ONLY, never stored in AppState (R8).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct NodeId { idx: u32, gen: u32 }

pub trait View {
    /// Per-node bookkeeping that survives rebuilds (Xilem ViewState):
    /// memo keys, cached measured-size handle. NOT app state (R4).
    type State;

    fn build(&self, cx: &mut BuildCx) -> (NodeId, Self::State);

    /// Diff self against prev; apply ONLY deltas to the retained node.
    /// Structural, type-driven; no virtual-DOM keyed matching at the leaf.
    /// Takes (tree: &mut RetainedTree, id: NodeId) — re-borrows per child,
    /// never nests two live &mut handles into the arena (see §14).
    fn rebuild(&self, prev: &Self, state: &mut Self::State,
               cx: &mut RebuildCx, tree: &mut RetainedTree, id: NodeId);

    fn teardown(&self, state: &mut Self::State, cx: &mut RebuildCx,
                tree: &mut RetainedTree, id: NodeId);

    // NOTE: there is deliberately NO `event` method (R7). Events route by
    // a non-recursive linear scan over derived hitboxes/focus_order (§6).
}
```

**[Resolved — rust-feasibility issue 1 (major) & issue 6: `View::event` is removed. The draft's `event(&self, &mut Self::State, &mut EventCx, NodeMut)` could not read `&mut AppState` and route a `Msg` without either re-introducing a borrow conflict or an `Rc`. Per-node key handling for text fields lives in `apply` keyed by `AppState.focus`, not in a tree walk. `rebuild`/`teardown` now take `(&mut RetainedTree, NodeId)` and re-borrow per child rather than holding a `NodeMut` across a recursive descent (see §14).]**

`view(&AppState, frame_now) -> impl View` is the single authoring entry point. It reads like immediate-mode:

```rust
fn view(s: &AppState, now: Instant) -> impl View {
    VStack::new((
        header(s),                          // RichRow of labels
        terminal_panel(s),                  // VT surface (populate_panel bridge)
        footer(s),
    ))
    .overlay(popup_overlay(s, now))         // Option<View>: the ONE open/closing popup (R14)
}
```

### How the diff against the retained tree works

- **Recursive `rebuild`, no central reconciler.** Each composite `View::rebuild` recursively calls children's `rebuild(tree, child_id)` — re-borrowing the arena per call (§14). `Label::rebuild` compares text and updates only if changed. No element-to-element keyed diff.
- **Identity.** Stable identity is `WidgetId` via id-path, recovered only at identity boundaries: focusable widgets, list items, panels (warpui: identity only at `ChildView`/`EntityId`, `presenter.rs:278-283`). The arena maps `WidgetId ↔ NodeId` per frame. We do **not** use React `key=`.
- **Variable-length children** (history rows, settings fields, popup lists) use the `ElementSplice` cursor — a single forward pass emitting `insert / mutate / skip / delete`, with generational slot identity. This is the minimal correct list diff and the home for list virtualization (§10).
- **Stale-safety.** Because routing is by linear hit-scan against the *current* frame's tree (not a captured path), a `Msg` produced last frame is always applied against current `AppState`. Where a deferred action could name a removed widget (e.g. a `Tab` whose target vanished on popup close), `apply` validates the `WidgetId` against the current focus-order and falls back deterministically (§7). No panic.

### What "dirty" means, and at what granularity

- **Granularity: per identity-boundary subtree**, keyed by `WidgetId`. Because there is ONE `AppState`, "dirty" is simply "AppState changed since last reconcile," and per-node `rebuild(prev,self)` field-diff localizes the work.
- **Incremental relayout applies to CHROME only.** A clean chrome subtree (unchanged `prev==self`) skips re-measure/re-place/re-paint and reuses last frame's cached instances. The **terminal grid re-emits its full visible window every frame** — there is no VT dirty-line API (verified: `RenderSnapshot` is a full clone, no damage info). The reused thing for the grid is the *shape cache* (glyph rasterization), not the instance stream. **[Resolved — rust-feasibility issue 3, capabilities lens issue 4: the "VT dirty-line info" input was fiction; the grid path is honest full-emit, cheap via caches.]**
- **One dirty signal** (R6): the coordinator owns a single `needs_reconcile`; `apply` sets it; the event loop drains once per frame.

---

## 5. Layout engine

### Scope (justified by consumers only — R13)

Consumers: header (HStack of labels, baseline-aligned, some right-aligned), footer (same), three popups (centered Block + VStack + List), terminal panel (fixed-rect VT surface), text field (in a popup). Primitive set:

- `HStack` / `VStack`
- `padding(insets)`, `gap(px)`
- alignment: `MainAxis::{Start, Center, End, SpaceBetween}`, `CrossAxis::{Start, Center, End, Stretch}` (only modes a consumer uses; `SpaceEvenly` deferred)
- sizing: `fixed(px)`, `flex(weight)`, `fill`, `center_in_parent`

This is **Flex-lite** (Flutter/warpui model, `elements/flex/mod.rs`), NOT CSS flexbox, NOT Taffy.

### The Element contract (constraints down, sizes up, positions down)

Adopted from warpui `elements/mod.rs:106-179`, realized as **index-based free functions over the arena** (NOT `&mut self` recursive methods — see §14):

```rust
// ILLUSTRATIVE SKETCH.
pub struct SizeConstraint { pub min: Vec2, pub max: Vec2 }  // logical px
impl SizeConstraint {
    fn apply(&self, size: Vec2) -> Vec2;            // = clamp
    fn child_constraint_along_axis(&self, axis: Axis) -> Self;
}

// measure: constraints in, size out (bottom-up). Free fn over the arena.
fn measure(tree: &mut RetainedTree, id: NodeId, c: SizeConstraint,
           fonts: &mut FontSystem, shape: &mut TextShapeCache) -> Vec2;

// place: parent assigns absolute origin (top-down). Positions live HERE only.
fn place(tree: &mut RetainedTree, id: NodeId, origin: Vec2);

// paint: emit instances at placed origin. Free fn; borrows resources separately.
fn paint(tree: &RetainedTree, id: NodeId,
         atlas: &mut GlyphAtlas, fonts: &mut FontSystem, swash: &mut SwashCache,
         shape: &mut TextShapeCache,
         out_rects: &mut Vec<RectInstance>, out_glyphs: &mut Vec<GlyphInstance>,
         out_shadows: &mut Vec<ShadowInstance>);
```

**[Resolved — rust-feasibility issue 2 (major) & issue 5: the draft's `fn measure(&mut self, …, mcx: &mut MeasureCx)` with `MeasureCx` co-bundling a tree handle would not compile in a recursive arena walk (holding `&mut Node` across a child recurse aliases `tree.nodes`). The passes are now index-based free fns: copy the small per-node data (kind, child `Vec<NodeId>` — cheap to clone or `mem::take`), release the `tree` borrow, then recurse per child id with a fresh borrow. `FontSystem`/`TextShapeCache`/atlas/swash are separate `&mut` params, disjoint from `tree` at every level. This matches how `redraw` already threads them disjointly at the top level (`app.rs:1015-1035`).]**

Two passes (`measure → place`), then `paint`. We keep warpui's hard invariant as a `debug_assert`: **a Flex with flexible children or `MainAxisSize::Max` panics on an infinite main-axis constraint** (`flex/mod.rs:206-214`). We drop warpui's barely-used `after_layout` phase.

### The measure pass for variable-width text

Text is pixel-based / variable-width (not monospace). It borrows `&mut FontSystem` + `&mut TextShapeCache`:

```rust
// A Text node measures via the migrated label helper (label.rs → term_ui, R9):
// measure_label_width(fonts, shape, text, font_size, scale_factor, weight, style) -> f32
let w = measure_label_width(fonts, shape, &text, font_size, scale_factor, weight, style);
let h = line_height(font_size);   // from FaceMetrics::cell_height
c.apply(vec2(w, h))
```

- Variable widths come from the shaper's per-glyph advance (`TextShapeCache::shape` → `ShapedLine.glyphs: Vec<LayoutGlyph>`). Parents clamp/position the returned box.
- **Text measurement cache.** Uses the existing `TextShapeCache` (two-tier char/string, family-scoped). `end_frame()` is called once per frame (R15). Functionally Warp's frame-double-buffered `LayoutCache`. A window resize that changes wrap width invalidates wrapped measurements; steady-state scroll/typing reuses everything.

### Incremental relayout (chrome only)

- A clean chrome identity-boundary subtree (its `view()` output equals last frame's) reuses cached measured size and placed boxes.
- **The terminal panel does NOT do per-line incremental relayout** (no VT dirty API). It re-emits the full visible grid every frame, bottom-anchored and clipped by `populate_panel`.
- **Scroll-preservation-on-remeasure is deferred with its consumer.** It is needed only for variable-height lists (the deferred §10 path); the history popup is fixed-row index-scroll and the VT grid is bottom-anchored. So it is explicitly **not** part of the now-built layout engine; it ships with the variable-height list if that consumer ever appears. **[Resolved — yagni-nodefer issue 6: the draft floated scroll-preservation between a built and a deferred section. It is now firmly in the deferred path.]**

---

## 6. Event routing + hit-testing

### Two-phase frame (R7)

1. **Event phase.** An OS/PTY/timer event arrives. term_ui hit-tests against the retained tree's **geometry only** via a **non-recursive linear scan** over the per-frame `hitboxes: Vec<(Bounds, WidgetId)>` (built during paint). Routing is a free fn — no tree walk, no `&mut AppState`:

   ```rust
   // ILLUSTRATIVE.
   fn route(tree: &RetainedTree, focus: Option<WidgetId>, ev: &RawEvent) -> Option<Msg>;
   ```
   Keyboard events route by `focus` (§7); pointer events resolve the topmost `(Bounds, WidgetId)` in z-order (overlay above base). A linear scan suffices for our element count (warpui's `Scene` R-tree `scene.rs:416-436` is overkill).
2. **Apply.** `apply(&mut AppState, Msg)` is the single mutation; sets the dirty signal. It may also drive a service (PTY write, renderer resize).
3. **Reconcile phase.** `view(&AppState, frame_now)` rebuilds the tree (immutable `&AppState`).

This collapses GPUI's capture/bubble dual-phase walk into an **ordered match in `route`**: global/modal handlers first (popup open?), then focused widget, then geometric hit.

### Pointer routing

- Mouse position → topmost `(Bounds, WidgetId)`. Click → `Msg::Click{ id, point }`.
- Hover: on mouse-move, recompute hovered `WidgetId`; if changed, `Msg::Hover(Option<WidgetId>)` → `AppState.hover`. We re-hit-test after each reconcile so hover updates when layout shifts under a stationary cursor (Warp `app.rs:622-623`).

### The gate cases (concrete mappings)

- **session-click-zone.** Becomes the header's session-label `WidgetId` in the per-frame hitbox list. A press inside its bounds → `Msg::CopySessionId`; `apply` writes `AppState.session_copied_until = Some(now)`; the green flash is *derived* from `frame_now < session_copied_until` (R12). No special-cased zone field.
- **click-outside-to-close.** The overlay popup registers its bounds as a `WidgetId`. A press whose hit `WidgetId` is NOT inside the popup subtree, while a popup is open, → `Msg::CloseAllPopups` — one ordered check in `route` *before* base-layer hit-testing (modal-first).
- **mouse-reporting-mode gate (specified against the real API).** The terminal panel's click routing branches on the emulator's `mouse_mode() -> MouseMode` (verified `emulator.rs:67`; variants `None`/`X10`/`ButtonEvent`/`AnyEvent`/`Sgr`, `grid.rs:131`), NOT a fictional boolean:

  ```rust
  // ILLUSTRATIVE.
  match emulator.mouse_mode() {
      MouseMode::None       => Msg::SelectionDrag { .. },   // local text selection
      MouseMode::X10        => Msg::PtyMouse(encode_x10(btn, cell)),
      MouseMode::ButtonEvent => Msg::PtyMouse(encode_sgr(btn, cell, press, /*drag=*/false)),
      MouseMode::AnyEvent    => Msg::PtyMouse(encode_sgr(btn, cell, press, /*drag=*/true)),
      MouseMode::Sgr         => Msg::PtyMouse(encode_sgr(btn, cell, press, /*drag=*/ /*per buttonmode*/)),
  }
  ```
  The encoders (`encode_x10`/`encode_sgr`) are pure fns (X10 vs SGR byte forms), unit-testable without GPU. **Mode coverage for now:** all five modes are forwarded (none deferred), because Claude Code's ink TUI may enable any of them and partial coverage would silently break mouse interaction inside the child app. `Msg::PtyMouse(bytes)` is written to the PTY service in `apply`; `Msg::SelectionDrag` mutates `AppState.selection` (bucket 1). Selection reuses term_gpu `Selection`/`CellPoint`/`expand_word`/`expand_line`/`selection_to_text`. **[Resolved — yagni-nodefer issue 4 (major): the draft gated on `mouse_reporting_enabled()` (does not exist) and a boolean that discards the X10/SGR/button-event/any-event distinction the PTY encoding needs. Respecified against the 5-variant `MouseMode` with pure encoders.]**

---

## 7. Focus system

### Focus as AppState (R11)

```rust
struct AppState { /* … */ focus: Option<WidgetId>, /* … */ }
```

One focused widget per window (Warp `window.rs:48`). No focus *ring object*, no per-element `on_action`, no up-the-tree bubble.

### Tab / Shift-Tab traversal order

- During reconcile, the engine collects focusable `WidgetId`s **in tree order** (depth-first) into `focus_order: Vec<WidgetId>` (bucket 2). Stable `WidgetId`s (R8) — not arena slots — so an item's identity survives reorder.
- `Tab` → `Msg::FocusNext`; `apply` sets `AppState.focus = next_focus(&order, cur)`. `Shift-Tab` → `Msg::FocusPrev`.
- **Determinism when `cur` is absent from `order`** (async tree change between frames): `next_focus` is total —

  ```rust
  fn next_focus(order: &[WidgetId], cur: Option<WidgetId>) -> Option<WidgetId> {
      match cur.and_then(|c| order.iter().position(|&w| w == c)) {
          Some(i) => order.get((i + 1) % order.len()).copied(),
          None    => order.first().copied(),   // cur removed/unknown ⇒ first focusable
      }
  }
  ```
  `apply` evaluates this against the **current** reconcile's `order`, never a stale captured vector. **[Resolved — capabilities lens issue 6: undefined-on-absent behavior is now total (fall to first); tied to current order, not stale.]**

### Modal focus trap + popup open/close transitions (now specified)

- When a popup is open, `focus_order` is collected from the popup subtree only (`if popup_open { order_within(popup_root) } else { order_within(root) }`).
- **On popup open** (`apply`): remember the prior focus in `AppState.focus_before_popup: Option<WidgetId>`, then set `AppState.focus` to the **first focusable in the popup** (or `None` if the popup has no fields, e.g. a pure list).
- **On popup close** (`apply`, after the close anim completes): restore `AppState.focus = focus_before_popup.take()` (validated against the current root `focus_order`; falls back to `None` if that widget no longer exists). **[Resolved — capabilities lens issue 6: open/close focus transitions were unspecified; the trap could leave focus pointing outside the visible popup. Now both transitions are explicit and validated.]**

### Focusability declaration

A widget opts in by carrying a `WidgetId` and `focusable: bool` (or being a known focusable type — `TextField`, list rows). Reconcile reads this to build `focus_order`. Non-focusable widgets are skipped. **[Rejected — yagni-nodefer issue 9: the GPUI JSON-keymap "optional data table" clause is removed entirely. Focus traversal is a fixed Tab/Shift-Tab index step; configurability has no consumer in the 5 capabilities or chrome list. Deleted per R13/KISS.]**

---

## 8. Text input (controlled component)

### The field widget is stateless; state is in AppState

```rust
struct TextFieldState {
    value: String,                      // bucket 1 — authoritative content
    caret: usize,                       // BYTE index into value — bucket 1
    selection: Option<(usize, usize)>,  // in-field selection (byte range) — bucket 1
    caret_epoch: Instant,               // bucket 1 — anim timer; blink DERIVED (R12)
}
// keyed in AppState by STABLE WidgetId (R8), never an arena slot:
// fields: HashMap<WidgetId, TextFieldState>
```

`caret` and `selection` are **byte indices**; caret *movement* respects grapheme boundaries (see policy below). **[Resolved — capabilities lens issue 5, rust-feasibility issue 7: byte/grapheme ambiguity removed (storage = bytes, movement = graphemes); the map is keyed by stable `WidgetId` so a structural reorder never orphans a field's caret/value.]**

### Caret ↔ pixel mapping (the load-bearing detail, now specified)

`measure_label_width` (`label.rs:79`) returns only a total width and is **insufficient** for caret placement. The field shapes its value once per reconcile into a `ShapedLine` (whose `glyphs: Vec<LayoutGlyph>` carry per-glyph `x`/`w` and cluster byte offsets — `text.rs:162-163`) and **retains that `ShapedLine` in bucket 2**. Two pure helpers (added to the migrated `label.rs`, R9) map both directions:

```rust
// ILLUSTRATIVE — pure fns over LayoutGlyph cluster/x/w.
/// caret byte index -> pixel X (to draw the caret rect).
fn caret_x(shaped: &ShapedLine, byte: usize) -> f32;
/// click X -> caret byte index (to place caret / start selection).
fn byte_at_x(shaped: &ShapedLine, x: f32) -> usize;
```

**Grapheme policy:** caret movement (`MoveCaretLeft/Right`, word/line granularity) snaps `caret` to grapheme-cluster boundaries (via `unicode-segmentation`-style boundaries over `value`); `byte_at_x` returns the nearest cluster boundary, never a mid-cluster byte. `caret_x` reads the `LayoutGlyph.x` of the cluster starting at `byte`. **[Resolved — capabilities lens issue 5 (major): caret-from-click and caret-X were asserted but unmapped. Now there is a named helper contract over `LayoutGlyph`, a stated grapheme policy, and an explicit note that the field retains the `ShapedLine` (bucket 2) because `measure_label_width` discards per-glyph positions.]**

### Edits are AppState mutations

A keystroke routed to `AppState.focus`'s field becomes a `Msg` (`InsertChar`, `Backspace`, `MoveCaret{dir,granularity}`, `SelectWordLeft`, `Paste`, …). `apply` mutates `value`/`caret`/`selection` via pure fns (`insert_char`, `move_caret`), each unit-testable without GPU. No widget-side mutation.

### Caret blink is DERIVED, not stored (R12)

```rust
fn caret_visible(epoch: Instant, frame_now: Instant) -> bool {
    let half_period_ms = 530;
    ((frame_now - epoch).as_millis() / half_period_ms) % 2 == 0
}
```

Any edit resets `caret_epoch = frame_now` (solid caret immediately after typing). The blink *animates* only because the ticker (§9) schedules a wake at each 530 ms boundary; that scheduling is specified in §9.

IME/composition is out of scope for current consumers (popup inputs). A `TextEvent` hook is noted (§16-Q6) but not built (YAGNI; Warp's IME path `event.rs:44-45` is the reference).

---

## 9. Animation

### Targets/timers in AppState, advanced by a ticker

```rust
enum AnimId { PopupOpen(WidgetId), PopupClose(WidgetId) }   // the ONLY consumers now
enum Easing { Linear, EaseInOut, EaseOut }                  // the ONLY curves used now
enum AnimChannel { Opacity }                                // what is animated (see below)

struct ActiveAnim {
    id: AnimId,
    channel: AnimChannel,
    start: Instant,        // bucket 1
    duration: Duration,
    easing: Easing,
    from: f32, to: f32,    // for Opacity: 0.0..=1.0
}
struct AppState { /* … */ animations: Vec<ActiveAnim>, /* … */ }
```

**Enumerated, not "copy from GPUI":** the only animated consumers are popup open/close. The only channel is **Opacity** (no scale/offset — see below). The only curves are `Linear`/`EaseInOut`/`EaseOut`. Momentum scroll already exists separately in term_gpu `scroll.rs` (`ScrollVelocity`, `decay_velocity`, `MOMENTUM_FRAME_INTERVAL=8ms`) and is reused as-is — it is not part of this `ActiveAnim` registry. **[Resolved — yagni-nodefer issue 5 (major): the easing/AnimId/from-to taxonomy is now concrete and consumer-scoped, not a placeholder pointing at another codebase.]**

- **Current eased value is derived** (R12): `delta = easing.apply((frame_now - start)/duration); alpha = lerp(from, to, delta)` — computed during paint, never stored. Easing fns are pure free fns in term_ui with unit tests.

### How a derived alpha reaches the GPU (the mechanism, now specified)

`RenderLayer` carries **no** opacity/transform field (verified `instances.rs:187-190`: `{shadows, rects, glyphs}` borrowed slices); `GpuRenderer::render` has **no** global alpha (`renderer.rs:177`). The ONLY mechanism is to **multiply the eased alpha into `color[3]` of every instance in the overlay**, every frame, in a named overlay-assembly pass:

```rust
// ILLUSTRATIVE — runs AFTER the popup subtree paints at base colors.
fn apply_overlay_alpha(alpha: f32,
                       rects: &mut [RectInstance],
                       glyphs: &mut [GlyphInstance],
                       shadows: &mut [ShadowInstance]) {
    for r in rects   { r.color[3] *= alpha; }
    for g in glyphs  { g.color[3] *= alpha; }
    for s in shadows { s.color[3] *= alpha; }
}
```

**This re-emits all overlay instances every animating frame — it is NOT incremental.** That is acceptable and explicitly stated: the popup instance count is tiny, and §4's "clean subtree reuses cached instances" therefore **does not apply to an animating popup** (an animating popup is never clean). The two sections are reconciled here: incremental relayout is a steady-state chrome optimization; during a transition the overlay is fully re-emitted with a fresh alpha. **[Resolved — capabilities lens issue 1 (blocker): "set the overlay's alpha" is not a primitive. The per-frame `color[3]` bake is now a named function with the derived-value contract, and the contradiction with §4's incremental claim is explicitly resolved (animating ⇒ not incremental).]**

**Color/emoji glyph limitation (stated, not hidden).** The text fragment shader returns color glyphs (emoji) at full alpha, bypassing `in.color.a` (verified `text.wgsl:65-67`: `if is_color { return sample; }`); only mono glyphs honor `in.color.a` (line 71). Therefore **opacity transitions are scoped to emoji-free chrome.** Today's popup chrome uses U+2192-style mono arrows, so this is satisfied. The design **forbids placing color/emoji glyphs inside an animated (fading) region**; if a future consumer needs to fade emoji, it requires a one-line term_gpu prerequisite change (premultiply: `if is_color { return sample * in.color.a; }`) which must be landed first. This is a real, documented capability limitation. **[Resolved — capabilities lens issue 2 (major): the emoji-vs-fade tension is now acknowledged. We choose option (a) — scope animation to emoji-free chrome — and document the term_gpu change as the prerequisite for the alternative, rather than resting the fade on a shader that defeats it.]**

**No scale/offset.** The draft's "slight scale/offset on the overlay" is **dropped**: `RenderLayer` has no transform, and the `scroll_offset_y` uniform is single-valued across BOTH layers (verified) so it cannot offset a popup independently of the terminal base. Animation is **opacity-only**. If scale/offset is ever wanted, it must be specified as a paint-time recomputation of the popup subtree's instance positions (driven by the eased delta, explicitly non-incremental, explicitly not via any uniform) — not built now (YAGNI). **[Resolved — capabilities lens issue 3 (major): scale/offset had no GPU substrate and the uniform could not be repurposed. Dropped to opacity-only; the future path is documented if a consumer appears.]**

### The ticker (next-wake scheduling, now specified)

The coordinator schedules the next frame while any of: `!animations.is_empty()`, a field is focused (blink), or momentum is live. The **next-wake instant** is the minimum of the relevant deadlines, NOT a fixed cadence:

```rust
fn next_wake(state: &AppState, now: Instant) -> Option<Instant> {
    let mut deadlines = Vec::new();
    // momentum: 8ms cadence (existing schedule_momentum_loop)
    if state.scroll_velocity.is_some() { deadlines.push(now + Duration::from_millis(8)); }
    // active opacity anims: next 8ms frame while running
    if !state.animations.is_empty()   { deadlines.push(now + Duration::from_millis(8)); }
    // caret blink: the NEXT 530ms boundary off caret_epoch (NOT 8ms — no busy loop)
    if let Some(f) = focused_field(state) {
        let elapsed = now - f.caret_epoch;
        let next_boundary = ((elapsed.as_millis() / 530) + 1) * 530;
        deadlines.push(f.caret_epoch + Duration::from_millis(next_boundary as u64));
    }
    deadlines.into_iter().min()
}
```

An idle focused field wakes every 530 ms, not every 8 ms. Completed anims are removed in `apply` on the tick that crosses `delta >= 1.0`. When nothing is pending, the ticker stops (no idle repaints). Reuses the existing `schedule_periodic_redraw`/`schedule_momentum_loop` timer factories emitting `UserEvent::{MomentumTick,TickRedraw}`. **[Resolved — capabilities lens issue 8: the blink-wake scheduling was hand-waved between §8/§9; the next-wake fn is now explicit and avoids a busy loop, living next to `caret_visible`.]**

### Popup transitions (no tree-as-truth; R4-clean)

Popup lifecycle is modeled in **AppState**, not in the tree:

```rust
enum PopupPhase { Open, Closing { anim: AnimId }, Closed }
```

`Msg::ToggleBackendPopup` flips to `Open` and pushes a `PopupOpen` anim. Close pushes a `PopupClose` anim and sets `Closing{anim}`; **`view(&AppState, frame_now)` emits the popup (with the derived fading alpha) as long as the popup is `Open` OR `Closing`.** Teardown happens only after `apply` observes the `PopupClose` anim crossing `delta >= 1.0`, removes it, and flips the popup to `Closed`. The tree never decides visibility; destroy+rebuild at any instant reproduces the same fading frame (R4). **[Resolved — single-source lens issue 2 (blocker): the draft's "deferred teardown in the tree" made the retained tree authoritative of popup visibility, breaking R4 and R6. The closing popup now lives in AppState as a `Closing` phase; the "deferred teardown in the tree" mechanism is removed entirely.]**

Warp has no animation engine — it's DIY `Instant`-in-handle + `repaint_after` (`shimmering_text.rs:274`). We centralize it in `AppState`, simpler given one state.

---

## 10. Virtualization

### Terminal scrollback (the VT grid) — built now

The VT grid *is already* the virtualization: render the visible row range directly with pixel offsets via term_gpu `populate_panel` (bottom-anchored, clipped to `panel_rect`, **bakes `scroll_offset_y_logical` into instance positions**). We do NOT wrap it in a generic list. `ScrollState` (bucket 1) drives the baked offset.

**Scroll mechanism (corrected).** Scroll is baked into terminal-surface instance positions by `populate_panel`/`build_cursor_rect`/`push_selection_rects`; `GpuRenderer::render` is passed `scroll_offset_y = 0.0` and the uniform is **NOT used by term_ui** (using it would scroll chrome too — single uniform, both layers, verified). Consequence for incremental relayout: **a scrolling terminal re-emits its visible-row instances every frame** (their baked positions change). The reused thing across frames is the **shape cache** (glyph rasterization), not the instance stream. **[Resolved — capabilities lens issue 4 (major): the draft's §10 implied the uniform path and implied instances are reused while scrolling; both corrected. The grid re-emits each frame; the height/shape cache is what is reused.]**

### History popup list — built now, fixed-row index model (NOT SumTree)

The history popup scrolls by **integer row index** (`HistoryDialogState::Visible{ scroll_offset: usize }`, `history/state.rs:17`) over fixed-height rows, capped at `MAX_VISIBLE_ROWS = 14`, bounded to a few hundred entries. The visible range is a trivial fixed-row slice:

```rust
fn history_visible_range(scroll_offset: usize, total: usize) -> Range<usize> {
    let start = scroll_offset.min(total);
    start..(start + MAX_VISIBLE_ROWS).min(total)
}
```

No `SumTree`, no `measured_count` estimation, no `invalidate_height_for_index`, no O(log n) pixel↔index seek — **none of that machinery has a consumer.** For fully-known fixed-row heights R4 holds exactly. **[Resolved — yagni-nodefer issue 2 (major), single-source lens issues 3 & 4, capabilities lens issue 7: the SumTree variable-height design is cut from the now-built §10; it was zero-consumer and (when lazy) R4-violating. The history list keeps its existing index model.]**

### Variable-height list (SumTree) — DEFERRED behind a named seam

If Claude's *streamed scrollback* ever becomes a term_ui `List` of variable-height rows (§16-Q4), the upgrade is **local to the `history_visible_range` / `visible_range` seam**: swap the fixed-row slice for the Warp `SumTree<ListItem{height:Option<Pixels>}>` model (`viewported_list.rs:482-508`) with lazy measurement, scroll-preservation-on-remeasure (`viewported_list.rs:44-51`), and the R4 weakening recorded in §3's caveat. Not built now (YAGNI).

### Recycling

Items are **rebuilt every frame from the render closure** for the visible range only (Warp `build_items`, `uniform_list.rs:57-70`). "Recycling" = out-of-range items are never constructed; no object pool.

### How it stays pure-cache (R4)

The realized window and emitted instances are bucket 2; for fixed-row content, destroy them and re-derive from `scroll_offset` (bucket 1) + viewport + content → identical result. The only authoritative scroll fact is in bucket 1 (the index `usize` for history, the pixel `ScrollState` for the grid).

---

## 11. Widget catalog

Scoped to current consumers (R13); all 5 capabilities (R11) are designed now.

| Widget | Measures | Paints (term_gpu primitives) | Reads from AppState |
|---|---|---|---|
| **Text** | `measure_label_width(...)` × line height | `push_label(...)` → `GlyphInstance`s | none (content passed by `view()`) |
| **Block** (bg/border/shadow/padding) | child size + insets | `ShadowInstance` under `RectInstance` bg + border rects | none (style is view-time) |
| **HStack / VStack** | Flex-lite (§5) | nothing; lays out children | none |
| **List** (selectable; fixed-row virtualizable) | visible-range items only (§10) | child rows; selection-highlight `RectInstance`; scrollbar `RectInstance` | history `scroll_offset:usize` (bucket1), selected index, `focus` |
| **RichRow** (styled spans, some right-aligned) | sum of span widths; right-aligned spans placed from the right edge | per-span `push_label` | label inputs (session id, backend names, req counts — *strings derived* per R12 via `header_labels(...)`) |
| **Separator** | fixed thickness × fill | one thin `RectInstance` | none |
| **Spacer / Fill** | `fixed(px)` / takes remaining | nothing | none |
| **TextField** (§8) | `measure_label_width` of value; caret X via `caret_x(shaped, byte)` | value glyphs; selection-highlight `RectInstance`; caret `RectInstance` (visible per `caret_visible`) | `TextFieldState`, `focus` |
| **TerminalSurface** | fixed `PanelRect` from BSP | `populate_panel(...)` + `build_cursor_rect(...)` + `push_selection_rects(...)` | `scroll`, `selection`; reads emulator (3-T) flags via the §6 gate |

Capability coverage check (R11): animations → popup open/close opacity transitions (§9, opacity-only, emoji-free); focus/traversal → TextField + List rows declare focusable (§7); text fields → TextField with `caret_x`/`byte_at_x` (§8); incremental relayout → chrome subtrees (§5); virtualization → fixed-row List + the grid-as-window (§10); retained scroll across frames → `ScrollState`/`scroll_offset` in bucket 1.

**Text widget weight/style surface — kept, justified.** RichRow renders styled spans (header/footer mix label styles, and popups emphasize the active selection), so the `weight: Weight` / `style: Style` plumbing inherited from `label.rs` has a real consumer. If verification at Phase C shows all chrome is a single weight, the plumbing is dropped from the Text widget surface until a styled consumer exists. **[Resolved — yagni-nodefer issue 7 (minor): the weight/style generality is conditionally justified; a Phase-C check decides whether to keep it.]** **[Phase C check (2026-05-29): the ported chrome is single-weight (NORMAL everywhere) and distinguishes the Session-copied flash by COLOUR, not weight — so `uikit::Segment` carries only `{text, color}`, no weight/style. The Text widget's `weight`/`italic` plumbing is RETAINED (not dropped) because the Phase D popups emphasize the active/selected row, the still-pending styled consumer; the keep/drop decision moves to D.]**

Widgets NOT built now (no consumer): Button-with-press-state, Checkbox, Table, generic Scrollable container, tooltips. Each is a small addition when a consumer appears (YAGNI).

---

## 12. term_ui crate layering

### What term_ui imports from term_gpu (consumes, never duplicates — R9)

From the verified public surface (`lib.rs`):
- **Instances & submission:** `RectInstance`, `GlyphInstance`, `ShadowInstance`, `RenderLayer`, `GpuRenderer` (`render`, `atlas_mut`, `resize`, `scale_factor`, `set_scale_factor`).
- **Text infra:** `FontSystem`, `SwashCache`, `TextShapeCache`, `ShapedLine` (for caret mapping), `FontFamily`, `FaceMetrics`, `Weight`, `Style`, `rasterize_glyph`.
- **VT bridge (read-only):** `populate_panel`, `build_cursor_rect`, `measure_cell_metrics`, `CellMetrics`, `PanelRect`, `DEFAULT_FG`, `CURSOR_COLOR`.
- **Scroll & selection:** `ScrollState`, `ScrollVelocity`, `decay_velocity`, momentum constants; `Selection`, `CellPoint`, `expand_word`, `expand_line`, `selection_to_text`, `push_selection_rects`.
- **Atlas:** held opaquely via `renderer.atlas_mut()`, threaded into helpers.

### The ONE migration: `label.rs` → term_ui (plus two new caret helpers)

`push_label` / `measure_label_width` move into term_ui (zero VT coupling, UI text — `label.rs:1-2`). The two new pure caret helpers `caret_x`/`byte_at_x` (§8) are added here (they operate on `ShapedLine`, also UI text). term_gpu's `label.rs` re-export is dropped; term_gpu shrinks to "GPU substrate + the VT bridge"; term_ui owns all chrome text shaping.

### The crate boundary

```
term_gpu  =  GPU substrate (instances, atlas, pipelines, renderer, text caches)
             + ScrollState/Selection math + the ONE VT bridge (populate_panel/build_cursor_rect)
                    ▲ consumed read-only
term_ui   =  retained+reactive engine: View trait, reconciler, arena, layout (Flex-lite),
             event routing/hit-test (linear scan), focus, text-field logic + caret mapping,
             animation registry (opacity), fixed-row virtualization, widget catalog, label.rs (migrated)
                    ▲ consumed
anyclaude =  thin coordinator (resources + AppState) + view(&AppState, frame_now) + pure logic fns
```

term_ui does NOT depend on `term_core` (VT parser) or `term_layout` (BSP) except through the `PanelRect`/`RenderSnapshot` types the VT bridge exposes — keeps the engine VT-agnostic. No `mvi` dependency (R1).

---

## 13. GpuApp dissolution map

### The new thin coordinator (resources + AppState — R10)

```rust
struct App {                       // replaces GpuApp; winit ApplicationHandler lives here
    // ── bucket 1: the one source of UI-decision truth ──
    state: AppState,
    // ── bucket 2: retained tree (owned arena) ──
    tree: RetainedTree,            // §14
    // ── bucket 3-S: resources / services ──
    renderer: Option<GpuRenderer>,
    window: Option<Arc<Window>>,
    fonts: FontSystem,
    swash: SwashCache,
    chrome_shape: TextShapeCache,  // SansSerif
    grid_shape: TextShapeCache,    // Monospace
    pty: Option<ChildPty>,
    // ── bucket 3-T: terminal-protocol authoritative content ──
    emulator: Option<Box<dyn TerminalEmulator>>,
    // ── bucket 3-S continued ──
    clipboard: Box<dyn Clipboard>,
    backend_state: BackendState,
    subagent_backend: AgentBackendState,
    teammate_backend: AgentBackendState,
    observability: ObservabilityHub,
    settings_manager: ClaudeSettingsManager,
    proxy: EventLoopProxy<UserEvent>,
    spawn_command: String, spawn_args: Vec<String>, spawn_env: Vec<(String,String)>,
    momentum_abort: Option<AbortHandle>, gesture_end_abort: Option<AbortHandle>,
    periodic_tick_abort: Option<AbortHandle>,
    start_time: Instant,
}
```

`window_event`/`user_event` do exactly: produce `Msg` → `apply(msg)` → `reconcile_and_render()`. The *ordered fan-out* of a `Msg` (resize, scale change) lives in `apply`, not the handler (R10).

### AppState struct + sub-structs

```rust
struct AppState {
    scale_factor: f32,
    grid_size: (usize, usize),
    modifiers: ModifiersState,
    scroll: ScrollState,                          // total_size_px re-derived from 3-T
    scroll_velocity: Option<ScrollVelocity>,
    cursor_pos: Option<(f32, f32)>,
    dragging_selection: bool,
    selection: Option<Selection>,
    last_click: Option<LastClick>,
    hover: Option<WidgetId>,
    session_id: String,
    session_copied_until: Option<Instant>,
    focus: Option<WidgetId>,
    focus_before_popup: Option<WidgetId>,         // §7 trap restore
    fields: HashMap<WidgetId, TextFieldState>,    // keyed by STABLE WidgetId (R8)
    animations: Vec<ActiveAnim>,                  // §9 (opacity only)
    backend_switch: BackendSwitchUi,
    history: HistoryUi,                           // scroll_offset: usize (row index)
    settings: SettingsUi,                         // fields + dirty + confirm_discard (bucket 1)
    // PTY phase = coordinator's `pty: Option<ChildPty>` (Option A); no extra field.
}
```

### Where each current GpuApp field/method goes

- **Fields → bucket 1:** `scale_factor`, `grid_size`, `modifiers`, `scroll`, `scroll_velocity`, `cursor_pos`, `dragging_selection`, `selection`, `last_click`, `session_id`, `session_copied_until`, + new `hover`, `focus`, `focus_before_popup`, `fields`, `animations`, popups.
- **Fields → bucket 2:** `cell_metrics`, `session_click_zone` (derived caches; move into `RetainedTree`).
- **Fields → bucket 3-S:** `renderer`, `window`, `font_system`, `swash_cache`, `shape_cache`, `ui_shape_cache`, `palette`, `pty`, abort handles, `clipboard`, `backend_state`, `subagent_backend`, `teammate_backend`, `observability`, spawn params, `settings_manager`, `start_time`, `proxy`.
- **Field → bucket 3-T:** `emulator` (authoritative terminal-protocol content, NOT a handle — R3).
- **Methods → pure logic fns** (unit-testable, no GPU): `fit_grid`, `cell_at` (inverse-row math — highest-value test target), `bump_click_count`, `terminal_panel_rect`, scroll-bounds half of `refresh_scroll_geometry`, `header_labels`, the mouse-report encoders (`encode_x10`/`encode_sgr`, §6), `caret_x`/`byte_at_x`/`move_caret`/`insert_char` (§8), `next_focus` (§7), the easing fns (§9). **Newly extracted:** `should_follow(scroll, eps) -> bool` extracted from the inline `was_at_bottom` check in `drain_pty` (`app.rs:324`); paste-payload assembly extracted from `paste_into_pty` (`app.rs:789`). **[Resolved — dissolution issue 4: the draft named `should_follow`/`build_paste_payload` as existing methods; they do not exist. Follow logic is inline in `drain_pty`; paste logic is inline in `paste_into_pty`. Corrected to "extract from these methods." Momentum naming aligned to the real `on_wheel`/`on_gesture_end`/`on_momentum_tick`.]**
- **Methods → thin mutation wrappers** (call pure fn, write via `apply`): `sync_grid_to_window`, `refresh_scroll_geometry`, `drain_pty`, `cancel_momentum`/`cancel_gesture_end`, `on_wheel`, `on_gesture_end`, `on_momentum_tick`, `on_cursor_moved`, `on_mouse_press`/`release`, `restart_pty`, `copy_session_id`, `update_modifiers`, the `toggle_*`/`close_all_popups`/`apply_*`/`handle_*_key` family.
- **Methods → view layer:** `redraw` → `view()` + reconcile; `chrome.rs` (`draw_header`/`draw_footer`) → header/footer `RichRow` views; `popup.rs` (`draw_*_popup`) → popup views.
- **Methods → resource/lifecycle (stay):** `schedule_once`/`schedule_periodic_redraw`/`schedule_momentum_loop`/`make_clipboard`, `request_redraw`, `copy_selection`/`paste_into_pty`.

### Orphaned concerns now given a home **[Resolved — dissolution issue 1 (major), issue 7]**

- **Diagnostic dump (Cmd+Shift+D).** Verified live: `window_event` matches `KeyCode::KeyD if shift_key()` → `diagnostic::dump_snapshot(grid_size, scroll.offset_y, scroll.max_offset(), snap)` (`app.rs:1359-1367`). New `Msg::DumpDiagnostic`; `apply` performs a no-op on `AppState` and routes to the diagnostic service (3-S), reading the emulator (3-T) snapshot and writing stderr. `diagnostic.rs` survives unchanged (it takes borrowed pieces, no `&self`). Tied to `feedback_capture_pty_bytes_for_render_bugs` — must not be dropped.
- **Cmd+Q / CloseRequested.** `KeyCode::KeyQ` (`app.rs:1368`) and `WindowEvent::CloseRequested` (`app.rs:1285`) → `event_loop.exit()` stay in the coordinator's `window_event` as lifecycle (not a `Msg`; they terminate the loop).
- **encode_key (keyboard → PTY bytes).** The existing key-encoding path stays as a pure fn producing PTY bytes; `apply` of `Msg::Key` (when focus is the terminal surface, not a popup field) writes the encoded bytes to the PTY service. Cursor-keys-app mode is read from the emulator (3-T).
- **Resize / ScaleFactorChanged ordered fan-out.** `Msg::Resize(size)` / `Msg::ScaleFactorChanged(sf)`; `apply` owns the ordered sequence (verified `app.rs:1286-1303`): for resize → `renderer.resize()` then recompute `grid_size`; for scale → set `scale_factor`, `renderer.set_scale_factor()`, **invalidate `cell_metrics = None`**, recompute grid. **[Resolved — dissolution issue 5: this ordered logic lives in `apply`, consistent with the clarified R10.]**
- **bootstrap::run() / construction.** `bootstrap.rs` survives mostly intact as resource construction (config load, settings manager, debug logger, tokio runtime, proxy server, teammate shim, spawn params, winit `EventLoop` + `UserEvent` proxy). `GpuApp::new` → `App::new` builds the bucket-3 handles and the initial `AppState`, dropping the three `Store::new` lines (`app.rs:244-246`). The EventLoop/UserEvent proxy plumbing is unchanged. **[Resolved — dissolution issue 7.]**

### The 3 popups: Store → plain AppState + pure fns (porting tests)

`scope.reduce(|s| f(s))` is already a pure transition — porting is renaming.

- **backend_switch.** `BackendSwitchState` enum → `BackendSwitchUi` (drop `impl mvi::State`). Each `BackendSwitchIntent` arm → a pure fn: `open`, `close`, `next_section`, `navigate(dir)`, `clear`; the existing free fns `navigate`/`wrap_around` port unchanged. Callers (`toggle_backend_switch_popup`, `handle_backend_switch_key`, `apply_backend_switch_selection`) call these instead of `store.dispatch(...)`. **New** `tests/backend_switch.rs` covers `wrap_around`, `next_section`, `clear`, `open`.
- **history.** `HistoryDialogState` enum + `HistoryEntry` + `MAX_VISIBLE_ROWS=14` + `scroll_offset: usize` kept (index model, §10). Arms → `load(entries)`, `close`, `scroll_up`, `scroll_down`. **Test ports (both files):** `tests/history_actor.rs` (6 tests) ports verbatim (`Store::new(HistoryActor,…)` → `HistoryDialogState::default()`; `dispatch(HistoryIntent::Load{entries})` → `history::load(entries)`) → renamed `tests/history.rs`; **`tests/history_state.rs` (2 tests, `use anyclaude::ui::history::HistoryDialogState`) is folded into `tests/history.rs`** since `HistoryDialogState` keeps its shape + `is_visible()`. **[Resolved — dissolution issue 2 (major), yagni-nodefer issue 8: `tests/history_state.rs` was omitted from the draft's port list, which would dangle/break the build. Now explicitly accounted for.]**
- **settings.** `SettingsDialogState` enum kept (`SettingsFieldSnapshot` lives in `crate::config`, not mvi). The Visible state's `fields`/`dirty`/`confirm_discard` are bucket-1 authoritative editing state (§3); `dirty` stays **stored** to match ported behavior. Arms → `load`, `close`, `request_close` (port the dirty/confirm-discard branch exactly; flag it latent-dead-but-tested since the keymap never dispatches it), `move_up`, `move_down`, `toggle`. `tests/settings_actor.rs` ports verbatim (**14 tests**, not 16) → renamed `tests/settings.rs`. There is no `settings_state.rs`. **[Resolved — dissolution issue 3, yagni-nodefer issue 10: corrected the count from 16 to the verified 14.]**
- **Files deleted:** `src/ui/{backend_switch,history,settings}/{state,intent,actor,mod}.rs` collapse into one plain module each (e.g. `src/ui/popups/{backend_switch,history,settings}.rs` = enum + pure fns), removing all `use mvi::…` and the three `Store::new` lines.

### PtyActor reborn as plain PTY-phase state

**Option A (recommended, confirmed):** the PTY MVI trio is dead — `PtyActor` is referenced nowhere outside `src/ui/pty/` + its tests (verified). The live model is `pty: Option<ChildPty>` + `emulator: Option<Box<dyn TerminalEmulator>>` with `restart_pty` doing fire-and-forget respawn. So:
- Delete `src/ui/pty/{state,intent,actor,mod}.rs` and `tests/pty_actor.rs` (**24 tests**) + `tests/pty_state.rs` (**3 tests**, testing `PtyLifecycleState::{default,is_ready,is_buffering}`).
- **Cost made visible (not framed as pure dead-code removal):** these 27 tests encode a buffering/lifecycle spec (`PtySideEffect::FlushBuffer`, `Pending/Attached/Ready/Restarting` transitions). Under Option A that behavior is **intentionally abandoned** because no running code depends on it (`PtyActor` is never constructed). If a "buffer input until Claude's first banner" requirement appears, **Option B** re-introduces a `PtyPhase{Pending,Attached,Ready,Restarting}` field in `AppState`, ports the arms to pure fns (`FlushBuffer` becomes a return value), and re-creates the tests from the deleted files as a frozen spec — not built now (YAGNI). **[Resolved — yagni-nodefer issue 11, dissolution issue 3: the 27-test deletion's cost is now explicit, and the count is corrected.]**
- "PTY lifecycle" in `AppState` stays as `pty: Option<ChildPty>` (None during the brief restart window) — that `Option` *is* the plain state.

### Net effect on `mvi`

After the popup port (Option A on PTY), the only `mvi::Store`/`Actor` users in `src/` are gone. The `mvi` crate becomes dead and is dropped from the workspace (R1). The memory note flagging it "preserved per mandate" is superseded by the §16-Q1 sign-off.

---

## 14. Rust ownership strategy

### Who owns the retained tree

An **arena owned by the coordinator** (`App.tree: RetainedTree`), NOT a graph of `Rc<RefCell<Node>>` (R8):

```rust
struct RetainedTree {
    nodes: Vec<Node>,                    // arena; slot = NodeId.idx
    free: Vec<u32>,                      // recycled slots
    gen: Vec<u32>,                       // generation per slot (stale-safe NodeId)
    id_map: HashMap<WidgetId, NodeId>,   // stable id-path ↔ arena slot, rebuilt per reconcile
    root: Option<NodeId>,
    dirty: HashSet<WidgetId>,            // touched on each controlled edit
    // derived caches (bucket 2):
    hitboxes: Vec<(Bounds, WidgetId)>,   // rebuilt each paint
    focus_order: Vec<WidgetId>,          // rebuilt each reconcile (stable WidgetIds)
}
struct Node {
    widget_id: Option<WidgetId>,         // identity only at boundaries (§4)
    kind: NodeKind,                      // measured size, placed origin, children: Vec<NodeId>
    view_state: ViewStateErased,
}
```

Children are `Vec<NodeId>` (arena indices), so the tree is a flat `Vec` — no `Rc`, no `RefCell`, single owner.

### No `NodeMut` held across a recursive descent

`rebuild`/`measure`/`place`/`paint` take **`(tree: &mut RetainedTree, id: NodeId)` (or `&RetainedTree` for paint/place reads)** and **re-borrow per child call** — they never hold a `&mut Node` across a recurse. The pattern:

```rust
// ILLUSTRATIVE — the arena idiom that compiles.
fn rebuild_node(tree: &mut RetainedTree, id: NodeId, /* view inputs */) {
    // 1. copy out what we need; release the borrow on `tree`:
    let children: Vec<NodeId> = std::mem::take(&mut tree.nodes[id.idx as usize].kind.children);
    // 2. mutate this node's own fields:
    // ... tree.nodes[id.idx as usize].kind.size = ...;  tree.dirty.insert(widget_id);
    // 3. recurse per child with a FRESH &mut borrow each call:
    for &child in &children {
        rebuild_node(tree, child, /* ... */);
    }
    // 4. put children back:
    tree.nodes[id.idx as usize].kind.children = children;
}
```

Dirtiness is recorded by inserting into `tree.dirty` on each edit (the "controlled handle records dirtiness" idea, realized as a method that pushes to the set rather than a borrow-the-whole-arena `NodeMut`). **[Resolved — rust-feasibility issue 6 (minor): `NodeMut = &mut RetainedTree + NodeId` borrows the entire arena; nesting a parent's and a child's `NodeMut` is two `&mut` to the same `Vec` and will not compile. Replaced with per-edit re-borrow + a `dirty` set, matching Xilem/Masonry `WidgetMut` discipline.]**

### How an event handler mutates the single AppState while the tree is borrowed

It does **not** — that is R7's two-phase split. Borrow timeline within one frame:

1. **Event phase:** `&App.tree` (immutable) → linear hit-scan → owned `Msg`. Borrow ends.
2. **Apply phase:** `&mut App.state` (tree NOT borrowed) → `apply(&mut state, msg)` (may also `&mut` a service). Borrow ends.
3. **Reconcile phase:** `&App.state` (immutable) + `&mut App.tree` + `&mut App.fonts`/`chrome_shape`/`grid_shape` borrowed together — **distinct fields of `App`, so field-splitting in `reconcile_and_render(&mut self)` gives the borrow checker disjoint mutable borrows** without `RefCell`. This is exactly how `redraw` already threads `&mut renderer.atlas_mut()`, `&mut self.font_system`, `&mut self.swash_cache`, `&mut self.shape_cache` disjointly today (`app.rs:1015-1035`). Borrow ends.

No phase holds a `&AppState` write while the tree is `&`-borrowed. Async PTY bytes and timer ticks enter as `UserEvent` → `Msg` → apply, never mutating `AppState` from a background thread. The only cross-thread state is the proxy-shared `BackendState`/`AgentBackendState` (`Arc<RwLock>`), a 3-S service, explicitly NOT part of the UI tree.

### The measure/paint borrow sequence (explicit ordering)

Glyph emission needs a **four-way disjoint `&mut`** (`renderer.atlas_mut()`, `&mut FontSystem`, `&mut SwashCache`, `&mut TextShapeCache`) writing into owned `Vec`s; then `render()` needs `&mut renderer` again (atlas upload is inside `render`, `renderer.rs:184`). These cannot overlap. The per-frame sequence:

1. **measure** [`&mut tree`, `&mut fonts`, `&mut shape`]
2. **place** [`&mut tree` only]
3. **paint** [`&tree` (read placed origins) + `&mut atlas`(=`renderer.atlas_mut()`), `&mut fonts`, `&mut swash`, `&mut shape` → append to owned `Vec<…>`]
4. **overlay alpha bake** [`&mut Vec<…>` only — §9 `apply_overlay_alpha`]
5. **drop all the above borrows**, then `renderer.render(RenderLayer{&shadows,&rects,&glyphs}, overlay, 0.0)` [`&mut renderer`]

Output `Vec`s are owned by `App` (reused scratch buffers across frames) so lifetimes stay trivial. The overlay alpha bake (step 4) must complete before `render` (step 5) since R14's single overlay is assembled from those same `Vec`s. **[Resolved — rust-feasibility issue 5 (minor): the draft's "paint emits Vecs then slice into RenderLayer" elided that paint cannot be a method on the renderer and that `atlas_mut()` writes must finish before `render` re-borrows the renderer. The sequence is now explicit; paint is a free fn, not a `PaintCx` co-bundling a tree handle.]**

### Identity via arena/WidgetId vs Rc<RefCell>

- **Default: arena + stable `WidgetId` (AppState-facing) + generational `NodeId` (bucket-2 only).** No interior mutability. Satisfies R8.
- **Justified interior mutability:** **none is required.** `TextShapeCache`/`SwashCache`/`GlyphAtlas` are `&mut`-threaded. The proxy-shared `BackendState`/`AgentBackendState` are `Arc<RwLock<…>>` because the proxy thread also reads them — a genuine cross-thread shared service (3-S), pre-existing, NOT part of the UI tree. If a future `Rc<RefCell>` need arises, R8 requires it be justified in writing here first.

---

## 15. Migration plan

Each phase leaves a **GREEN build** (`cargo check` after every commit, `cargo test` at milestones — per memory) and decomposes into atomic commits. No phase deletes `mvi` until its last consumer is gone (Phase F). **`frame_now` is threaded as an explicit reconcile input from Phase B onward** so the R4 property test can freeze it.

**Phase A — term_ui core engine, proven on a toy.** New crate `crates/term_ui`. Implement: arena `RetainedTree` (with `WidgetId↔NodeId` map + `dirty` set), `View` trait (build/rebuild/teardown — no `event`), recursive index-based reconciler with per-child re-borrow, `ElementSplice` list diff, Flex-lite layout (index-based measure→place→paint free fns), the migrated `label.rs` + `caret_x`/`byte_at_x`. Prove with `examples/toy.rs` rendering a static `VStack(Text, HStack(Text,Text))` through a real `GpuRenderer`, and the **R4 property test** (rebuild-from-scratch == incremental, for a fixed `(AppState, window, frame_now)`, comparing geometry+color+glyph-identity while normalizing atlas UVs/frame counters, OR offscreen-framebuffer compare into a fresh atlas). *Atomic commits:* arena+id-map; View trait; reconciler; splice; flex measure; flex place; flex paint; label+caret migration; toy example; R4 property test. **Milestone gate:** R4 test green.

**Phase B — AppState + coordinator skeleton.** Add `AppState` (window/grid/scroll first), the `App` coordinator, the two-phase frame (`event→Msg→apply→reconcile`), the single dirty signal, the ticker with `next_wake`, and `frame_now` threading. Render an empty frame + hardcoded chrome stub. No popups, no PTY yet. *Atomic commits:* AppState skeleton; coordinator; Msg/apply; reconcile-and-render; ticker+next_wake.

**Phase C — port chrome (header/footer).** Move `chrome.rs` into `header_labels` pure fn (tested) + `RichRow` views. Verify whether chrome uses multiple weights/styles (decides the §11 weight/style plumbing). Wire session-click-zone as a hitbox `WidgetId`; session flash derived from `session_copied_until`. Wire scroll (`ScrollState`) + momentum (baked into instance positions, uniform = 0.0). *Atomic commits:* `header_labels`+test; header view; footer view; session-click hitbox; scroll wiring.

> **As built (2026-05-29, `135fcb2`→`1dcf6f2`).** The chrome VIEWS shipped; the layering came out cleaner than the sketch:
> - **A new `uikit` crate** holds the *generic, domain-agnostic* bars — `Segment {text, color}` + `header_bar`/`footer_bar` over term_ui (`C.0`/`C.1`, 4 layout tests, fault-injection-verified). **No `RichRow` widget was needed** (YAGNI win): plain `Stack`/`Text`/`Spacer` compose the row, and the 1px fence is a `Block` (bg-fill) wrapping a `Spacer`, pinned `Sizing::Fixed(1.0)` + `CrossAxis::Stretch`; the footer right-aligns its version via `Spacer::fill()`.
> - **`header_labels` became `ui::chrome_labels` in anyclaude `src/`** (`C.2`, 5 tests), NOT in the kit. The presenter owns the *words* ("backend:/sub:/team:/Reqs:/Uptime:/Session:" + the copied-flash colour) so `uikit` stays domain-free; it takes primitives only, so it is pure and GPU-free. (Diverged from the literal "header_labels pure fn in the kit" — putting app vocabulary in a reusable kit would pollute it.)
> - **Proven via `examples/chrome_preview.rs`** (`C.3`) — the real chrome through a real `GpuRenderer` on the Phase B coordinator pattern, user-verified.
> - **Deferred to the real coordinator:** the session-click hitbox (needs R7 event routing / hit-testing) and scroll+momentum. Those land when `GpuApp` is actually replaced by an anyclaude-`src/` coordinator, not by the preview. Palette constants are duplicated in `chrome_labels` with a cutover TODO (unify when the legacy `ui::gpu::chrome` is deleted, Phase E/F).

**Phase D — port popups, delete their actors.** Per §13: collapse each popup into enum + pure fns; rewire callers; port `tests/history_actor.rs` + fold `tests/history_state.rs` → `tests/history.rs`; port `tests/settings_actor.rs` (14) → `tests/settings.rs`; add `tests/backend_switch.rs`. Build the three popup views as the single overlay (R14) with opacity open/close transitions (§9, `Closing` phase in AppState, emoji-free). Wire focus trap + open/close focus transitions (§7), TextField (controlled, §8) with `caret_x`/`byte_at_x` for any popup input, caret-blink derivation + 530 ms wake. Delete `src/ui/{backend_switch,history,settings}/{state,intent,actor,mod}.rs` and the three `Store::new` lines. *Atomic commits (×3 popups):* port state+fns; port/fold tests; build view; delete old module. Then: focus trap+transitions; TextField+caret mapping. **Milestone gate:** all ported popup tests green; manual verify popups open/close/navigate/animate (per `feedback_verify_before_docs`).

**Phase E — port terminal surface + PTY-phase-as-plain-state.** `TerminalSurface` view over `populate_panel`/`build_cursor_rect`/`push_selection_rects` (scroll baked in, uniform 0.0). Selection via term_gpu `Selection`. Mouse-reporting gate over `MouseMode` with `encode_x10`/`encode_sgr` (§6). `encode_key` → PTY for terminal-focused keys (cursor-keys-app from 3-T). Follow-mode via extracted `should_follow`. `Msg::Resize`/`Msg::ScaleFactorChanged` ordered fan-out in `apply`. `Msg::DumpDiagnostic` (Cmd+Shift+D) + Cmd+Q/CloseRequested lifecycle. PTY phase = `Option<ChildPty>` (Option A); delete `src/ui/pty/*` + `tests/pty_actor.rs` (24) + `tests/pty_state.rs` (3) with the explicit cost note (§13). Wire `restart_pty`, paste (extracted payload), copy. *Atomic commits:* terminal view; selection wiring; mouse-report gate+encoders; encode_key wiring; follow-mode; resize/scale fan-out; diagnostic+quit; delete pty MVI+tests; restart/paste/copy. **Milestone gate:** full app renders + interacts; verify against a real Claude Code session.

**Phase F — delete mvi crate.** Remove `crates/mvi` from the workspace; confirm `grep -r mvi src/ crates/` is empty (R1). Update workspace `Cargo.toml`. *Atomic commit:* drop mvi crate. **Milestone gate:** full `cargo test` green; R1/R9 grep checks pass.

Ordering rationale: A proves the engine in isolation; B–E migrate consumers one at a time, each leaving the old path working until its replacement is green; F is pure deletion.

---

## 16. Open questions (resolved or still needing the user)

1. **mvi deletion sign-off.** **[Assumed granted per the AGREED DECISIONS preamble — "The mvi crate and the dead PtyActor get deleted (with user sign-off)." This doc treats sign-off as obtained; if the mandate is not lifted, R1/§13/Phase F revert.]**
2. **PTY phase: Option A vs B.** **[Resolved: Option A — delete dead lifecycle; `Option<ChildPty>` is the state. Confirmed `PtyActor` is constructed nowhere. The 27-test cost is documented (§13); Option B re-introduces the 4-state machine only if a "buffer until first banner" requirement appears.]**
3. **`WidgetId` strategy.** **[Resolved — not actually open: keying bucket-1 state (`fields`, `focus`, `animations`, `hover`) by an arena slot would orphan state on reorder (R8 / rust-feasibility issue 7). WidgetId = stable id-path; arena `NodeId` = internal bucket-2 slot. Two distinct types.]**
4. **History list scale.** **[Resolved for now: the history popup keeps its fixed-row index model (≤ few hundred rows, 14 visible); no SumTree is built. STILL NEEDS THE USER only if Claude's *streamed scrollback* is meant to become a virtualized variable-height term_ui `List` — that triggers the deferred SumTree path behind the `visible_range` seam, with the R4 weakening recorded in §3.]**
5. **Themeable background / clear color.** **[STILL OPEN: term_gpu hardwires clear color `(0.04,0.04,0.06)` in `renderer.rs` and duplicates it as `DEFAULT_BG` in `panel_render.rs`. Does term_ui need a themeable background now (⇒ reconcile the duplication + expose a clear-color setter), or stay hardwired? No current consumer; default is stay hardwired per YAGNI.]**
6. **IME / composition.** **[STILL OPEN, default defer: out of scope for popup-only fields. A `TextEvent` hook is designed (§8) but not built until a real multi-byte-input field appears (Warp `event.rs:44-45` as reference).]**
7. **Accessibility hook.** **[STILL OPEN, default omit: no consumer. Omit entirely now; if needed, reserve a no-op hook in the paint pass.]**
8. **Color/emoji in animated regions.** **[New, default resolved: opacity transitions are scoped to emoji-free chrome (§9), because the text shader returns color glyphs at full alpha. If a future consumer must fade emoji, land the one-line term_gpu premultiply (`return sample * in.color.a;`) first. Confirm this limitation is acceptable for current chrome (today's popups use mono arrows, so it is).]**