//! `ChildSessionManager` — the registry + lifecycle for child Claude sessions
//! (the teammates shown in the right overlay).
//!
//! It is the IDENTITY / lifecycle authority, deliberately separate from the
//! [`PanelManager`] (which is pure UI-decision truth — order / focus /
//! visibility) and from the process layer (a future `ChildPtySpawner`). It owns
//! the registry — the `PaneId ↔ PanelId` bimap that the tmux control plane needs
//! — and REACTS to typed [`ChildSessionEvent`]s by orchestrating the panel
//! manager (create a panel on registration, remove it on exit). It does NOT
//! parse tmux (that's the future `TmuxAdapter`, which translates the shim's
//! commands into these events) and does NOT spawn processes (that's the future
//! `ChildPtySpawner`, which a `Register` will delegate to once panels host live
//! terminals). MODEL≠VIEW: the registry lives here (a coordinator collaborator);
//! the `PanelManager` it drives stays UI truth in `AppState`.
//!
//! Milestone-A scope: registry + `Register`/`Unregister` against placeholder
//! panels. Surfaces (the live terminal per pane) and the richer events
//! (`Input`/`Resize`/`SetTitle`, the synchronous reply channel) land with the
//! spawner + control plane. See `docs/design/multi-instance-panels.md`.

use std::collections::HashMap;

use crate::ui::panel_manager::{PanelId, PanelKind, PanelManager};

/// External identity of a pane — the tmux `%N` the control plane hands back to
/// Claude Code. Monotonic, never reused, so a held id can't re-address a
/// different pane after one is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u64);

/// What a registering child announces about itself: its display identity plus
/// the command to run in its pane. (The tmux control plane fills `command` from
/// `send-keys … claude …`; the debug emitter uses a shell.)
#[derive(Debug, Clone, PartialEq)]
pub struct ChildSpec {
    /// Display title (agent / teammate name).
    pub name: String,
    /// Accent colour (agent colour), RGBA in 0..=1.
    pub accent: [f32; 4],
    /// Program to spawn into the pane's PTY.
    pub command: String,
    /// Arguments to `command`.
    pub args: Vec<String>,
    /// Extra environment for the child.
    pub env: Vec<(String, String)>,
}

/// One registry entry: a registered child and the panel that mirrors it. The
/// live `TerminalSurface` (emulator + PTY) is added in the resources milestone.
#[derive(Debug, Clone)]
pub struct ChildSession {
    pub pane_id: PaneId,
    pub panel_id: PanelId,
    pub name: String,
    pub accent: [f32; 4],
}

/// A typed, tmux-agnostic lifecycle event the manager reacts to. The
/// `TmuxAdapter` (control plane) is one producer of these; a debug emitter is
/// another. New variants (`Input`/`Resize`/`SetTitle`, replies) land with the
/// later milestones.
#[derive(Debug, Clone, PartialEq)]
pub enum ChildSessionEvent {
    /// A child session registered → create a pane/panel for it.
    Register(ChildSpec),
    /// A child session ended → remove its pane/panel.
    Unregister(PaneId),
    /// A pane was retitled (`tmux select-pane -T`) → update its panel title.
    SetTitle { pane: PaneId, title: String },
}

/// The registry of child sessions. Reacts to [`ChildSessionEvent`]s, orchestrating
/// the passed [`PanelManager`]; holds the `PaneId ↔ PanelId` bimap.
#[derive(Debug, Default)]
pub struct ChildSessionManager {
    registry: HashMap<PaneId, ChildSession>,
    /// Issues the next `PaneId`; monotonic.
    next_pane: u64,
}

impl ChildSessionManager {
    pub fn new() -> Self {
        Self { registry: HashMap::new(), next_pane: 0 }
    }

    /// React to one lifecycle event, driving `panels`. `Register` returns the new
    /// [`PaneId`] (the control plane replies it to the shim as `%N`).
    pub fn apply(
        &mut self,
        event: ChildSessionEvent,
        panels: &mut PanelManager,
    ) -> Option<PaneId> {
        match event {
            ChildSessionEvent::Register(spec) => Some(self.register(spec, panels)),
            ChildSessionEvent::Unregister(pane) => {
                self.unregister(pane, panels);
                None
            }
            ChildSessionEvent::SetTitle { pane, title } => {
                self.set_title(pane, &title, panels);
                None
            }
        }
    }

    /// Register a child: mint a `PaneId`, create its panel, record the mapping,
    /// and show the overlay — a registered child means there's something to see
    /// (symmetric with `unregister` hiding it when the last child leaves).
    fn register(&mut self, spec: ChildSpec, panels: &mut PanelManager) -> PaneId {
        let pane_id = PaneId(self.next_pane);
        self.next_pane += 1;
        let panel_id = panels.create(PanelKind::Teammate, spec.name.clone(), spec.accent);
        panels.set_visible(true);
        self.registry
            .insert(pane_id, ChildSession { pane_id, panel_id, name: spec.name, accent: spec.accent });
        pane_id
    }

    /// Unregister a child: drop the registry entry and remove its panel. When the
    /// last child leaves, close the overlay (nothing to show). No-op if the pane
    /// is unknown.
    fn unregister(&mut self, pane: PaneId, panels: &mut PanelManager) {
        if let Some(session) = self.registry.remove(&pane) {
            panels.remove(session.panel_id);
            if self.registry.is_empty() {
                panels.set_visible(false);
            }
        }
    }

    /// Retitle a pane: update the registry entry's name and its panel's title.
    /// No-op if the pane is unknown.
    fn set_title(&mut self, pane: PaneId, title: &str, panels: &mut PanelManager) {
        if let Some(session) = self.registry.get_mut(&pane) {
            session.name = title.to_string();
            panels.set_title(session.panel_id, title);
        }
    }

    // ── queries (the bimap the control plane / input routing read) ──────────

    /// The panel mirroring `pane`, if registered.
    pub fn panel_for(&self, pane: PaneId) -> Option<PanelId> {
        self.registry.get(&pane).map(|s| s.panel_id)
    }

    /// The pane behind `panel`, if any (reverse of the bimap).
    pub fn pane_for(&self, panel: PanelId) -> Option<PaneId> {
        self.registry.values().find(|s| s.panel_id == panel).map(|s| s.pane_id)
    }

    pub fn get(&self, pane: PaneId) -> Option<&ChildSession> {
        self.registry.get(&pane)
    }

    pub fn len(&self) -> usize {
        self.registry.len()
    }

    pub fn is_empty(&self) -> bool {
        self.registry.is_empty()
    }
}
