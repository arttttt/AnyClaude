# Multi-Instance Panels (`PanelManager`) — Design Doc (DRAFT 2026-05-31)

> Status: **DRAFT (2026-05-31).** Architecture agreed in conversation; not yet implemented.
> This doc works out the *architecture* of showing multiple Claude instances inside
> anyclaude's GPU terminal — the long-term home for both the "teammates on the right"
> view and the "CLAUDES sidebar on the left" view from the product mockup.
>
> It assumes the term_ui stack as shipped: one `AppState`, the unified
> `Msg`/`apply`/`Effect` loop (E.8), retained term_ui chrome/popup trees, and the
> terminal grid drawn by direct `populate_panel` (R5). Invariant references (R1, R5,
> R10, R12, …) are to [`term-ui-design.md`](term-ui-design.md). Teammate routing /
> tmux background is in [`agent-teams-integration.md`](agent-teams-integration.md) and
> [`agent-team-routing.md`](agent-team-routing.md).
>
> **All Rust below is illustrative sketch only** — it shows shape and intent, not final
> API. Field names, signatures, and module layout are decided at implementation time.

---

## 0. Motivation & current state

Today the live `GpuApp` is **single-panel**: it spawns the main `claude` in one
`ChildPty` (`gpu/session.rs`) and renders that one VT emulator into the whole terminal
rect. Claude Code's *agent teams* feature drives teammate spawns through the `tmux`
binary, which a PATH shim (`src/shim/tmux.rs`) intercepts: the shim rewrites the
teammate's `ANTHROPIC_BASE_URL` for per-teammate backend routing and **forwards every
other tmux command to a real tmux server**. So teammate panes are created and drawn by
a real, separate tmux server — **not** by anyclaude's GPU terminal, and not visible as
native panels.

The target end-state (product mockup) is two distinct UI regions:

- a **left sidebar** listing top-level Claude sessions ("CLAUDES": `cw-main`,
  `ac-test`, …), which **displaces** the main content; and
- a **right overlay** holding the running teammates (child `claude` processes), which
  **floats over** the content, is **fully hideable**, has **resizable width**, and a
  **vertically-centered toggle/indicator button**.

This doc defines the architecture that serves both, starting from the right overlay.

---

## 1. Foundational principle — MODEL ≠ VIEW

The single most important decision. Two things are easy to conflate:

- **Instance model** — *which* `claude` processes exist, their identity, backend
  routing, lifecycle. Driven by Claude Code (via tmux commands) and by the user
  (the "new claude" action).
- **View / layout** — *how* we present them: tmux-style tiling (teammates on the
  right) **or** the sidebar switcher (CLAUDES list on the left).

These are different UX over the **same** model. Claude Code asks for tmux tiling; the
mockup wants a sidebar. If layout is baked into the model, switching views means
rewriting the model. Therefore: **one instance model, several views; the view is
anyclaude's product decision, not Claude Code's.**

### 1.1 Two-level instance model (north star)

```
anyclaude window
├── Session "cw-main"   ← top-level claude   (a LEFT-sidebar entry)
│   ├── pane %0  (main CC)                ┐
│   ├── pane %1  (teammate module-mapper) │ shown in the RIGHT overlay
│   └── pane %2  (teammate flow-tracer)   ┘
├── Session "ac-test"   ← another top-level claude
└── Session "rs-review"
        ▲
        └── switching sessions = the LEFT sidebar
```

- The **left sidebar** switches **Sessions** (top level — the mockup's CLAUDES list).
- The **right overlay** shows the **panes/teammates of the active Session** (what
  Claude Code spawns via tmux).
- Teammates are **owned by their Session** (model). The right `PanelManager` is the UI
  controller that renders/manages the *active session's* teammates; switching sessions
  re-points it at that session's teammate set.

Milestone 1 builds only the right-overlay machinery for a single session. Sessions and
the left sidebar come later but the model is shaped so they layer on without rework.

---

## 2. `PanelManager` — ONE class, TWO instances

The core reusable component. **It is a single concrete type with a single `impl`,
instantiated twice** — once per on-screen panel region. Not two types, not a trait with
two implementors, not a payload-generic specialization. Everything that differs between
the left and right panel is **data in the instance's `Policy`**, never the type.

```rust
// ILLUSTRATIVE — one type, one impl.
struct PanelManager {
    policy: Policy,            // all left/right differences live here (static)
    panels: Vec<Panel>,        // ordered; order = sort order
    focus:  Option<PanelId>,   // which panel this manager considers focused
    visible: bool,             // expanded vs collapsed
    width:  f32,               // current (arbitrary) width; remembered across collapse
    next_seq: u64,             // issues PanelId
}

impl PanelManager {
    fn create(&mut self, panel: Panel) -> PanelId { /* … */ }
    fn remove(&mut self, id: PanelId) { /* … */ }
    fn reorder(&mut self, /* sort key / explicit order */) { /* … */ }
    fn set_focus(&mut self, id: PanelId) { /* … */ }
    fn set_visible(&mut self, v: bool) { /* … */ }   // animates width 0↔width
    fn toggle(&mut self) { self.set_visible(!self.visible) }
    fn any_active(&self) -> bool { /* any panel with a running child */ }
    fn list(&self) -> &[Panel] { &self.panels }
}
```

The lifecycle/ordering logic (`create`/`remove`/`reorder`/`set_focus`/visibility/
`any_active`) is written **once** and branches only on `self.policy`.

### 2.1 `Panel` — one type for both managers

```rust
// ILLUSTRATIVE — a Panel is a Panel whether it wraps a Session or a teammate;
// the distinction is data, not a separate type.
struct Panel {
    id: PanelId,
    kind: PanelKind,                 // Main | Teammate | Session (data, not a type)
    title: String,                   // agent / session name
    accent: Color,                   // agent color
    agent: Option<AgentMeta>,        // agent-id / team — populated later from send-keys
    surface: Option<TerminalSurface>,// emulator + scroll + selection — None for placeholders
    running: bool,                   // is the child process alive — feeds any_active()
}
```

`surface`, `agent`, and child processes are **deferred** (see §8/§9). A Milestone-1
placeholder panel has `surface: None` and no process.

### 2.2 `Policy` — the only thing that differs

```rust
// ILLUSTRATIVE — static per instance, set at construction.
struct Policy {
    side: Side,                  // Left | Right
    placement: Placement,        // Displace | Overlay
    render: RenderMode,          // Switcher (one active) | Stack (all)
    resizable: bool,
    edge_toggle: bool,           // hosts the centered toggle/indicator button
    has_indicator: bool,
    min_width: f32,
    max_width: f32,
}

let left  = PanelManager::new(Policy { side: Left,  placement: Displace, render: Switcher,
                                       resizable: /*later*/ false, edge_toggle: false,
                                       has_indicator: false, .. });
let right = PanelManager::new(Policy { side: Right, placement: Overlay,  render: Stack,
                                       resizable: true, edge_toggle: true,
                                       has_indicator: true, .. });
```

| Aspect | **Left** (Sessions) | **Right** (Teammates) |
|---|---|---|
| Content | top-level claude sessions (CLAUDES list) | child-claude teammates |
| Placement | **Displace** — pushes content right | **Overlay** — floats over content |
| Simultaneously visible | **Switcher** — one active rendered | **Stack** — all rendered |
| Side / width | left, (later resizable) | right, **resizable (arbitrary)** |
| Hiding | collapses (later) | **fully hideable**, edge toggle button |
| Indicator | — | lit when `any_active()` |
| Timeline | later | near-term target |

Forward-building the left instance before it is fully used is intentional and allowed
(YAGNI removed for forward-built UI scaffolding — see `feedback_solid_dry_kiss_yagni`).

---

## 3. The right overlay in detail

Two of the right instance's fields are **dynamic state**, not policy:

- **`width`** — arbitrary, dragged via the overlay's inner edge (a divider-style
  handle), clamped to `policy.[min_width, max_width]` (the BSP clamp lesson: never let
  a panel degenerate to 0 or eat the whole window).
- **`visible`** — expanded (renders at `width`) vs collapsed (renders at 0, **`width`
  remembered**).

The **toggle/indicator button** sits at the vertical center of the overlay's inner
edge. Click → collapse/expand **to the current `width`**. It doubles as the activity
indicator: lit when `any_active()` (a live child-claude exists).

### 3.1 Geometry (expanded / collapsed)

```
EXPANDED (visible):
  content_rect = [window.left .. window.right − width]
  overlay      = [window.right − width .. window.right]

  ┌─ content ───────────────┬◉─ overlay (teammates, stacked) ─┐
  │   main grid %0           ││  teammate 1                    │
  │                          ││  teammate 2                    │
  └──────────────────────────┴────────────────────────────────┘
                              ▲
                ◉ = toggle/indicator button + drag handle,
                    vertically centered, on the edge x = window.right − width

COLLAPSED (!visible):  overlay width renders as 0, content full width,
  the button is pinned to the window's right edge (still clickable, still indicating):

  ┌─ content (full width) ──────────────────────────────────◉┐
```

"Fully hidden" means the panels/content are hidden but the **button persists** (a thin
edge handle) so the overlay can be re-expanded and the indicator stays visible. This is
the pill toggle from the mockup.

### 3.2 The inner edge is one interactive zone

The overlay's inner edge hosts **both** affordances: the vertical center is the toggle
button (+ indicator); the rest of the edge height is the resize drag handle. Hit-zones
for both are recomputed each frame from geometry (the same pattern as today's
`session_click_zone`).

### 3.3 Collapse/expand animation

`set_visible` animates `width` between `0` and the remembered `width` (and slides the
button to the edge), through the same `term_ui::anim` epoch mechanism used for popup
open/close fade (E.7). This is **not** YAGNI — a resizable toggle feels broken without
it; it is part of the UX (R11: animations are a designed-now capability).

---

## 4. Geometry & render pipeline

Layout authority is **the two `PanelManager`s + the content rect** — there is no BSP
tree (see §7). Per frame:

```
content_rect = window
  − left.occupied_width()    // Displace: reduces content from the left
  // Overlay (right) does NOT reduce content_rect — it floats over the right edge

BASE layer (R5 — grids drawn directly, never a retained view):
  populate_panel(content_rect, active_session.main_emulator, …)        // the main CC
  if right.visible {
    for (panel, rect) in right.stack_rects() {
      populate_panel(rect, panel.surface.emulator, panel.scroll, …)    // teammates (later)
    }
  }

OVERLAY layer (retained term_ui trees — reuses the E.6/E.7 machinery):
  panel_manager_view(&left)     // sidebar switcher: list + titles
  panel_manager_view(&right)    // overlay stack: frame + titles + edge button + indicator
  chrome_view(&AppState)        // unchanged
  popup_view(&AppState)         // unchanged
```

**Principle: grids are direct `populate_panel` (R5); frames/lists/indicator/buttons are
a view.** `panel_manager_view(&PanelManager)` is written **once** and branches on
`self.policy` to render either the sidebar switcher or the overlay stack. This is what
makes the mockup's sidebar nearly free later — the sidebar **is** `panel_manager_view`
of the left instance.

---

## 5. State & the unified loop (R10 holds)

```rust
// ILLUSTRATIVE
struct AppState {
    left:  PanelManager,    // policy = sidebar (Displace/Switcher)
    right: PanelManager,    // policy = overlay (Overlay/Stack)
    focus: FocusId,         // THE single focus (see §6)
    // unchanged globals: modifiers, input, popups(3), chrome/session …
}
```

The reuse rules hold: this is **one `AppState`** (R10) — the two managers are nested
collections of UI-decision truth, exactly like today's single-terminal scroll/selection
are AppState truth. Heavy resources (emulator/PTY handles) stay in the coordinator keyed
by `PanelId` (bucket 3-S/3-T), mirroring how the single `Session` holds them today.

New messages/effects extend the existing Elm loop (E.8); they do not replace it:

```rust
// ILLUSTRATIVE additions
enum Msg {
    // … existing …
    CreatePanel  { mgr: ManagerId, /* spec */ },
    RemovePanel  { mgr: ManagerId, id: PanelId },
    ReorderPanel { mgr: ManagerId, /* key */ },
    FocusPanel   { mgr: ManagerId, id: PanelId },
    ToggleManager{ mgr: ManagerId },               // collapse/expand
    EdgeDrag     { mgr: ManagerId, x: f32 },        // resize width (clamped)
    SwitchSession{ id: PanelId },                   // later (left sidebar)
}
enum Effect {
    // … existing …
    SpawnChild { panel: PanelId, /* cmd */ },       // later
    KillChild  { panel: PanelId },                  // later
    WriteToPty { panel: PanelId, bytes: Vec<u8> },  // panel-addressed (was unaddressed)
    ResizePty  { panel: PanelId, cols: u16, rows: u16 },
}
```

`ManagerId = Left | Right` addresses which instance a message targets; the generic
`create`/`remove`/`reorder`/`toggle` run in `apply` against the addressed manager. PTY-
addressed effects gain a `PanelId`; `Redraw` stays global.

---

## 6. Focus — a single field, everything derived

There is exactly **one** focus. Everything follows from it; there is no separate
"active session" vs "keyboard focus".

- **Keyboard → the focused terminal.** Whatever is focused (main CC or a teammate)
  receives keystrokes. Teammates are therefore **not** read-only — focusing one routes
  the keyboard to it.
- **"Active session" is derived from focus** (the session owning the focused panel),
  never stored separately (R12).
- Focus changes by clicking a panel (later, also a cycle hotkey).
- **Mouse scroll/selection target the panel under the cursor** via hit-test (as in the
  `term_grid` example) — this is a mouse concern, orthogonal to keyboard focus, and is
  resolved per-event, not stored. (If we later want scroll to follow focus strictly,
  that is a small change; default is cursor-targeted.)

This collapses a whole class of state: no `active_session`, no `keyboard_focus` — just
`focus`, with the rest derived.

---

## 7. What we do NOT use, and what we reuse

- **No BSP `term_layout::PanelTree` as layout authority.** An earlier draft proposed it;
  the two-`PanelManager` model replaces it. Layout is the managers' simple
  displace/overlay + list/stack geometry, not a binary-split tree. (`term_layout` stays
  example-only; it can be repurposed *inside* a manager if nested teammate tiling is
  ever needed — see the parked question §11.)
- **Reuse the per-panel terminal mechanics from `term_grid.rs`** (the proven multi-panel
  example): one `portable-pty` child per panel, a reader thread per panel signalling
  `EventLoopProxy::…(PanelId)`, per-panel emulator + scroll + selection, and
  `sync_panels_to_tree`-style deferred resize (destructive column shrink → resize on
  gesture release). This is the **lower** layer and is independent of BSP; we adopt it
  for §9, dropping only the BSP layout part.

---

## 8. Control plane — `tmux shim → /api/tmux/* → winit` (LATER)

How teammate spawns reach anyclaude. **Deferred past Milestone 1**, recorded here so M1
does not box it out.

The shim is a short-lived bash process per tmux invocation; it `curl`s anyclaude and
**blocks** on the response — because `split-window -P` must **synchronously return** the
new pane id, which Claude Code reads from stdout and uses in the next `send-keys -t %N`.

Decisions already taken (in conversation):

- **Transport: a new `/api/tmux/*` surface on the existing proxy.** The existing
  `teammate-start` endpoint is later moved/updated under this surface. (API work itself
  is a later milestone.)
- **Full emulation; remove the real-tmux fallback.** The shim emulates the ~12 verbs
  Claude Code actually issues (captured in a tmux-shim log): `display-message` → `@0`;
  `list-panes` → current `%N` from anyclaude's registry; `split-window -P` → create a
  panel, **return its `%N`**; `send-keys … claude …` → spawn that command into the
  panel's PTY (the shim already parses/rewrites the line — URL/headers/agent-id);
  `kill-pane` → close; `select-pane -T/-P` → title/accent; `resize-pane`/`select-layout`
  → geometry hints (interpreted by anyclaude as the layout authority, not followed
  verbatim); `show -gv …` → success. An **unknown verb is an explicit error + log**, so
  protocol drift in Claude Code is noticed (no silent forwarding).

Threading shape (the synchronous request/response across the tokio↔winit boundary):

```
shim (bash, curl, blocking)
  → POST /api/tmux/split-window {…}         (axum handler, tokio runtime)
       handler: push TmuxRequest{ op, reply: oneshot } onto a queue
                proxy.send_event(UserEvent::TmuxControl)   // wake winit
                reply_rx.await   →   HTTP body "%N"
  → winit user_event(TmuxControl):
       drain queue → Msg::Tmux(op) → apply → Effect(CreatePanel / SpawnChild / …)
       → perform_effects mutates the registry, formats "%N", sends it into the oneshot
```

This is the existing `BytesArrived`-wake pattern plus a `oneshot` for the reply. Note:
`UserEvent` becomes non-`Copy` (it carries the reply channel). The `%N ↔ PanelId` bimap
is anyclaude's pane registry (UI identity), stored as `Panel.tmux_id`.

**Open:** whether Claude Code needs `$TMUX` / `$TMUX_PANE` seeded for the main CC beyond
`--teammate-mode tmux` (the captured log shows it querying `%0`, so `%0` came from
somewhere). Resolved by experiment when §8 is built.

---

## 9. Per-panel resources (LATER)

Resources (bucket 3) live in the coordinator. A `Panes`/`Panels` collaborator replaces
the single `Session`:

```rust
// ILLUSTRATIVE
struct PanelResources {
    surfaces: HashMap<PanelId, PaneSurface>, // emulator + ChildPty + grid_size cache
    // spawn params (main + per-teammate)
}
// reader thread per panel → UserEvent::PtyBytes(PanelId) → drain → emulator
// sync_to_layout(): resize each emulator+PTY to its rect, debounced by grid_size,
//                   destructive shrink deferred to gesture release (term_grid lesson)
```

A direct port of the `term_grid` model into the coordinator. In Milestone 1 only the
main panel has a surface; placeholders have none.

---

## 10. Milestones

| # | Scope |
|---|---|
| **M1 — UI only** | The right `PanelManager` instance + `panel_manager_view`, rendering **placeholder** panels in a resizable overlay with the centered toggle/indicator button and collapse/expand animation. Manual (debug-only) controls to create/remove/reorder placeholders, resize, and toggle. The main CC grid renders in `content_rect`. **No `/api/tmux/*`, no child processes, no per-panel emulator.** The left instance is scaffolded (same class) but empty. |
| M2 — Resources | Per-panel `TerminalSurface` (emulator + PTY) via the `term_grid` port; teammate grids render live; panel-addressed input/scroll/selection; single `focus` routes keyboard. |
| M3 — Control plane | `/api/tmux/*` + the shim full-emulation cutover; `split-window`/`send-keys`/`kill-pane` drive real teammate panels; per-teammate routing folded in. |
| M4 — Sessions / left sidebar | The left instance goes live: top-level sessions, switcher, displace; `SwitchSession` re-points the right manager at the active session's teammates. |

### 10.1 Milestone 1 as an honest subset

| Layer | Full architecture | M1 |
|---|---|---|
| `PanelManager` class | left + right instances | **right** instance live; left empty scaffold |
| `Panel.surface` | emulator + PTY | `None` (placeholders) |
| Render | grids + 2 views | content grid as today + `panel_manager_view(right)` |
| Overlay | resizable + toggle + indicator + anim | **all of it** (UI is the point of M1) |
| Indicator | derived from running children | derived from placeholder count |
| Focus | single `focus`, keyboard-routed | single `focus`, visual highlight only |
| Control plane / child / API | §8 / §9 | — |

Nothing in M1 is rebuilt later — only extended (add `surface`, child, API).

---

## 11. Open questions (TBD)

1. **Internal layout of the right overlay** — a simple sortable **vertical stack** of
   teammates, or **nested tiling** inside the overlay? *Parked by the user; decide
   before M2.* If a stack, no BSP is needed anywhere; if nested tiling, `term_layout`
   may be repurposed inside the right manager only.
2. **`$TMUX` / `$TMUX_PANE` seeding** for the main CC (see §8) — resolve by experiment
   at M3.
3. **Scroll/selection targeting** — cursor-under (default, §6) vs follow-focus. Default
   stands unless revisited.

---

## 12. Invariant alignment

- **R1 (no MVI):** unaffected — this builds on the plain `AppState` + `apply` loop.
- **R5 (grid stays direct):** preserved — panel grids are `populate_panel` loops;
  frames/lists/indicator are views.
- **R10 (GpuApp = resources + one AppState + views + pure apply/effects):** preserved —
  two managers are nested AppState truth; resources stay in the coordinator; the loop is
  extended, not replaced.
- **R12 (derived, not stored):** the indicator (`any_active`) and "active session" are
  derived; only `panels`/`focus`/`visible`/`width` are stored.
- **R11 (capabilities designed now):** the overlay animation uses the existing
  `term_ui::anim` epoch — a designed-now capability, not a deferral.
