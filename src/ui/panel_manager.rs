//! `PanelManager` — one reusable type, instantiated once per on-screen panel
//! region (the left sessions sidebar and the right teammates overlay).
//!
//! There is a SINGLE concrete type with a SINGLE `impl`, created twice. Nothing
//! that differs between the left and right panel is encoded in the type: all of
//! it lives in the [`Policy`] data set at construction. The lifecycle / ordering
//! logic (`create`/`remove`/`reorder`/`set_focus`/visibility/`any_active`) is
//! written once and branches only on `self.policy`.
//!
//! This holds UI-decision truth only (R2): the ordered set of panels, the
//! focused panel, visibility, and the (resizable) width. The heavy resources —
//! a panel's VT emulator and PTY child — live in the coordinator keyed by
//! [`PanelId`] and are added in a later milestone; a Milestone-1 placeholder
//! panel has none. See `docs/design/multi-instance-panels.md`.

/// Stable identifier for a panel within a manager. Monotonic, never reused even
/// after a panel is removed, so a held id can't silently re-address a different
/// panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelId(pub u64);

/// Which on-screen manager a message / op targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerId {
    Left,
    Right,
}

/// Which screen side a manager occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// How a manager occupies space relative to the terminal content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Reduces the content rect from its side (the left sidebar pushes content
    /// over).
    Displace,
    /// Floats over the content without reflowing it (the right overlay).
    Overlay,
}

/// How many panels a manager renders at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// One active panel rendered at a time (a session switcher).
    Switcher,
    /// All panels rendered, stacked (the teammates overlay).
    Stack,
}

/// What a panel wraps. This is DATA, not a type — a `Panel` is a `Panel` whether
/// it stands for a top-level session or a teammate; the manager treats them
/// uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKind {
    /// The main Claude Code pane (`%0` in tmux terms).
    Main,
    /// A teammate child process (right overlay).
    Teammate,
    /// A top-level Claude session (left sidebar).
    Session,
}

/// Static per-instance configuration — the ONLY thing that differs between the
/// left and right managers. Set once at construction and never mutated; the
/// dynamic per-instance state (panels / focus / visibility / width) lives on
/// [`PanelManager`].
#[derive(Debug, Clone)]
pub struct Policy {
    pub side: Side,
    pub placement: Placement,
    pub render: RenderMode,
    /// Whether the inner edge can be dragged to change `width`.
    pub resizable: bool,
    /// Whether the inner edge hosts the centered collapse/expand toggle button.
    pub edge_toggle: bool,
    /// Whether the toggle button doubles as an activity indicator (lit while a
    /// child process is running).
    pub has_indicator: bool,
    /// Width clamp (logical px); `set_width` keeps `width` inside `[min, max]`.
    pub min_width: f32,
    pub max_width: f32,
    /// Initial width before the user resizes (logical px).
    pub default_width: f32,
    /// Width the overlay shrinks to when collapsed — the bare edge strip (the
    /// toggle button + drag handle). A hand-drag can shrink down to this, and
    /// releasing below `min_width` snaps to collapsed.
    pub collapsed_width: f32,
}

impl Policy {
    /// The left sessions sidebar: displaces content, shows one active session,
    /// not yet resizable (the left panel lands in a later milestone).
    pub fn sidebar() -> Self {
        Self {
            side: Side::Left,
            placement: Placement::Displace,
            render: RenderMode::Switcher,
            resizable: false,
            edge_toggle: false,
            has_indicator: false,
            min_width: 160.0,
            max_width: 480.0,
            default_width: 240.0,
            collapsed_width: 20.0,
        }
    }

    /// The right teammates overlay: floats over content, stacks every teammate,
    /// resizable with a centered toggle/indicator button on its inner edge.
    pub fn overlay() -> Self {
        Self {
            side: Side::Right,
            placement: Placement::Overlay,
            render: RenderMode::Stack,
            resizable: true,
            edge_toggle: true,
            has_indicator: true,
            min_width: 220.0,
            max_width: 900.0,
            default_width: 420.0,
            // Small so the collapsed pill (centred on the divider = the overlay's
            // left edge at `window.right - collapsed_width`) sits near the edge.
            collapsed_width: 14.0,
        }
    }
}

/// One panel object the manager creates / removes / reorders. UI-decision truth
/// only; the VT emulator + PTY child (when the panel becomes a live terminal)
/// live in the coordinator keyed by `id`.
#[derive(Debug, Clone)]
pub struct Panel {
    pub id: PanelId,
    pub kind: PanelKind,
    /// Display title (agent / session name).
    pub title: String,
    /// Accent color (agent color), RGBA in 0..=1.
    pub accent: [f32; 4],
    /// Whether this panel's child process is alive — feeds `any_active` (the
    /// indicator). Always `false` for a Milestone-1 placeholder.
    pub running: bool,
}

/// One class, two instances. Same type, same `impl`; behaviour branches on
/// `self.policy`.
pub struct PanelManager {
    policy: Policy,
    panels: Vec<Panel>,
    focus: Option<PanelId>,
    visible: bool,
    /// Current (resizable) width in logical px, remembered across collapse so a
    /// re-expand returns to it.
    width: f32,
    /// While `Some`, a hand-drag is in flight and this is the live rendered
    /// width (down to `collapsed_width`); the coordinator renders it directly,
    /// bypassing the collapse/expand animation. `None` outside a drag.
    drag_width: Option<f32>,
    /// Issues the next `PanelId`; monotonic.
    next_seq: u64,
}

impl PanelManager {
    /// Construct an empty, collapsed manager. It starts hidden so it has no
    /// effect on the layout until something is created and it is shown.
    pub fn new(policy: Policy) -> Self {
        let width = policy.default_width;
        Self {
            policy,
            panels: Vec::new(),
            focus: None,
            visible: false,
            width,
            drag_width: None,
            next_seq: 0,
        }
    }

    // ── queries ──────────────────────────────────────────────────────────

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn panels(&self) -> &[Panel] {
        &self.panels
    }

    pub fn len(&self) -> usize {
        self.panels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }

    pub fn focus(&self) -> Option<PanelId> {
        self.focus
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn width(&self) -> f32 {
        self.width
    }

    pub fn get(&self, id: PanelId) -> Option<&Panel> {
        self.panels.iter().find(|p| p.id == id)
    }

    /// Indicator source: any panel has a running child process (R12 — derived,
    /// computed on demand, never stored).
    pub fn any_active(&self) -> bool {
        self.panels.iter().any(|p| p.running)
    }

    // ── lifecycle ────────────────────────────────────────────────────────

    /// Append a panel and return its fresh id. The first panel created becomes
    /// the focused one.
    pub fn create(&mut self, kind: PanelKind, title: impl Into<String>, accent: [f32; 4]) -> PanelId {
        let id = PanelId(self.next_seq);
        self.next_seq += 1;
        self.panels.push(Panel { id, kind, title: title.into(), accent, running: false });
        if self.focus.is_none() {
            self.focus = Some(id);
        }
        id
    }

    /// Remove the panel with `id`. If it held focus, focus falls back to the
    /// first remaining panel (or `None` when empty).
    pub fn remove(&mut self, id: PanelId) {
        self.panels.retain(|p| p.id != id);
        if self.focus == Some(id) {
            self.focus = self.panels.first().map(|p| p.id);
        }
    }

    /// Move the panel at index `from` to index `to` (both clamped into range) —
    /// the explicit reorder/sort primitive. No-op when out of range or equal.
    pub fn reorder(&mut self, from: usize, to: usize) {
        if from >= self.panels.len() || self.panels.is_empty() {
            return;
        }
        let to = to.min(self.panels.len() - 1);
        if from == to {
            return;
        }
        let panel = self.panels.remove(from);
        self.panels.insert(to, panel);
    }

    /// Focus `id` if it exists. Returns whether it was found.
    pub fn set_focus(&mut self, id: PanelId) -> bool {
        if self.panels.iter().any(|p| p.id == id) {
            self.focus = Some(id);
            true
        } else {
            false
        }
    }

    /// Mark a panel's child as running / stopped (drives `any_active`).
    pub fn set_running(&mut self, id: PanelId, running: bool) {
        if let Some(p) = self.panels.iter_mut().find(|p| p.id == id) {
            p.running = running;
        }
    }

    // ── visibility + size ────────────────────────────────────────────────

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// Collapse ↔ expand. The width is preserved while collapsed so a re-expand
    /// returns to the same size.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Set the remembered expanded width, clamped to `[min_width, max_width]`
    /// (programmatic set; the hand-drag uses the `*_edge_drag` methods).
    pub fn set_width(&mut self, width: f32) -> f32 {
        self.width = width.clamp(self.policy.min_width, self.policy.max_width);
        self.width
    }

    // ── hand-drag resize ─────────────────────────────────────────────────
    // A drag controls the live rendered width from `collapsed_width` (fully
    // hidden) up to `max_width`, independent of `visible` so the overlay can be
    // dragged open from collapsed and dragged shut from expanded. `visible` +
    // the remembered `width` only change on release.

    /// The live drag width this frame, or `None` when not dragging. The
    /// coordinator renders the overlay at this width directly (no animation).
    pub fn drag_width(&self) -> Option<f32> {
        self.drag_width
    }

    /// Begin a hand-drag, seeded at the current rendered width (the expanded
    /// `width` when open, the bare `collapsed_width` when collapsed).
    pub fn begin_edge_drag(&mut self) {
        let start = if self.visible { self.width } else { self.policy.collapsed_width };
        self.drag_width = Some(start);
    }

    /// Update the live drag width, clamped to `[collapsed_width, max_width]`.
    pub fn edge_drag_to(&mut self, width: f32) {
        self.drag_width = Some(width.clamp(self.policy.collapsed_width, self.policy.max_width));
    }

    /// End a hand-drag: release at or above `min_width` expands (and remembers
    /// the new `width`); below it snaps collapsed, keeping the prior `width` for
    /// the next button-expand. No-op when no drag was in flight.
    pub fn end_edge_drag(&mut self) {
        if let Some(dw) = self.drag_width.take() {
            if dw >= self.policy.min_width {
                self.width = dw;
                self.visible = true;
            } else {
                self.visible = false;
            }
        }
    }
}
