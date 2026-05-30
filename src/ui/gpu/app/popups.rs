//! The three popup overlays (backend switch / history / settings): their
//! open-close toggles and the apply/save handlers that commit a popup's edits
//! back to the backend / settings managers.

use crate::config::{save_claude_settings, Config, SettingsFieldSnapshot};
use crate::ui::backend_switch::{
    override_selection_to_backend_id, BackendPopupSection, BackendSwitchIntent, BackendSwitchState,
};
use crate::ui::history::{HistoryEntry, HistoryIntent};
use crate::ui::settings::{SettingsDialogState, SettingsIntent};

impl super::GpuApp {
    /// Dispatch `Close` to every popup store. Called by the toggle handlers
    /// before opening a new popup; Esc / click-outside go through `apply`.
    fn close_all_popups(&mut self) {
        self.state.close_all_popups();
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Cmd+B handler — open or close the backend switch popup. Open
    /// dispatches the Open intent with the active backend pre-selected
    /// so pressing Enter is a no-op if the user is just inspecting.
    pub(super) fn toggle_backend_switch_popup(&mut self) {
        if self.state.backend_switch.is_visible() {
            self.close_all_popups();
            return;
        }
        let cfg = self.backends.backend_state.get_config();
        if cfg.backends.is_empty() {
            return;
        }
        let active = self.backends.backend_state.get_active_backend();
        let backend_selection = cfg
            .backends
            .iter()
            .position(|b| b.name == active)
            .unwrap_or(0);
        // Close any other open popup first.
        self.close_all_popups();
        self.state.backend_switch.apply(BackendSwitchIntent::Open {
            backend_selection,
            subagent_selection: 0,
            teammate_selection: 0,
            backends_count: cfg.backends.len(),
        });
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Cmd+H handler — open or close the history popup. The switch
    /// log is snapshotted into the popup at open time; subsequent
    /// switches only show up after the user reopens.
    pub(super) fn toggle_history_popup(&mut self) {
        if self.state.history.is_visible() {
            self.close_all_popups();
            return;
        }
        let entries = self.backends.backend_state.get_switch_log();
        let history_entries: Vec<HistoryEntry> = entries
            .into_iter()
            .map(|e| HistoryEntry {
                timestamp: e.timestamp,
                from_backend: e.old_backend,
                to_backend: e.new_backend,
            })
            .collect();
        self.close_all_popups();
        self.state.history.apply(HistoryIntent::Load {
            entries: history_entries,
        });
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Cmd+E handler — open or close the settings popup. Field
    /// snapshots are loaded from `settings_manager`; Space toggles
    /// rows (marks state dirty), Enter applies and saves, Esc
    /// discards.
    pub(super) fn toggle_settings_popup(&mut self) {
        if self.state.settings.is_visible() {
            self.close_all_popups();
            return;
        }
        let fields: Vec<SettingsFieldSnapshot> = self
            .backends.settings_manager
            .registry()
            .iter()
            .map(|def| SettingsFieldSnapshot {
                id: def.id,
                label: def.label,
                description: def.description,
                section: def.section,
                value: self.backends.settings_manager.get(def.id),
            })
            .collect();
        if fields.is_empty() {
            return;
        }
        self.close_all_popups();
        self.state.settings.apply(SettingsIntent::Load { fields });
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Persist the settings popup's edits to disk. Reads the current
    /// popup state, applies each row to the manager, then calls
    /// `save_claude_settings`. Errors are logged but non-fatal.
    pub(super) fn apply_settings_and_save(&mut self) {
        let fields = match &self.state.settings {
            SettingsDialogState::Visible { fields, .. } => fields.clone(),
            SettingsDialogState::Hidden => return,
        };
        for field in &fields {
            self.backends.settings_manager.set(field.id, field.value);
        }
        let snapshot = self
            .backends.settings_manager
            .snapshot_values()
            .into_iter()
            .map(|(id, v)| (id.as_str().to_string(), v))
            .collect();
        if let Err(e) = save_claude_settings(&Config::config_path(), &snapshot) {
            eprintln!("anyclaude: failed to save settings: {e}");
        }
    }

    /// Apply whichever action the active section maps to: the Active
    /// section calls `switch_backend`; the Subagent / Teammate sections
    /// write into their `AgentBackendState` (index 0 == Disabled
    /// → `None`, index N+1 == backend N). Errors are logged but
    /// non-fatal — the popup still closes.
    pub(super) fn apply_backend_switch_selection(&mut self) {
        let (section, backend_sel, subagent_sel, teammate_sel) =
            match self.state.backend_switch {
                BackendSwitchState::Visible {
                    section,
                    backend_selection,
                    subagent_selection,
                    teammate_selection,
                    ..
                } => (
                    section,
                    backend_selection,
                    subagent_selection,
                    teammate_selection,
                ),
                BackendSwitchState::Hidden => return,
            };
        let cfg = self.backends.backend_state.get_config();
        match section {
            BackendPopupSection::ActiveBackend => {
                if let Some(b) = cfg.backends.get(backend_sel) {
                    let id = b.name.clone();
                    if let Err(e) = self.backends.backend_state.switch_backend(&id) {
                        eprintln!("anyclaude: backend switch failed: {e}");
                    }
                }
            }
            BackendPopupSection::SubagentBackend => {
                let new_value = override_selection_to_backend_id(&cfg.backends, subagent_sel);
                self.backends.subagent_backend.set(new_value);
            }
            BackendPopupSection::TeammateBackend => {
                let new_value = override_selection_to_backend_id(&cfg.backends, teammate_sel);
                self.backends.teammate_backend.set(new_value);
            }
        }
    }
}
