use crate::config::{ClaudeSettingsManager, ConfigStore};
use crate::error::ErrorRegistry;
use crate::ipc::{BackendInfo, ProxyStatus};
use crate::metrics::MetricsSnapshot;
use crate::pty::PtyHandle;
use crate::ui::history::{HistoryDialogState, HistoryEntry, HistoryIntent, HistoryReducer};
use crate::ui::mvi::Reducer;
use crate::ui::pty::{PtyIntent, PtyLifecycleState, PtyReducer};
use crate::ui::selection::{GridPos, TextSelection};
use crate::ui::settings::{SettingsDialogState, SettingsIntent, SettingsReducer};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PopupKind {
    BackendSwitch,
    Status,
    History,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Terminal,
    Popup(PopupKind),
}

#[derive(Debug)]
pub enum UiCommand {
    SwitchBackend { backend_id: String },
    RefreshStatus,
    RefreshMetrics { backend_id: Option<String> },
    RefreshBackends,
    ReloadConfig,
    RestartPty {
        env_vars: Vec<(String, String)>,
        cli_args: Vec<String>,
        settings_toml: std::collections::HashMap<String, bool>,
    },
}

pub type UiCommandSender = mpsc::Sender<UiCommand>;

/// Generic MVI dispatch: takes current state, runs reducer, stores result.
macro_rules! dispatch_mvi {
    ($self:expr, $field:ident, $reducer:ty, $intent:expr) => {
        $self.$field = <$reducer>::reduce(std::mem::take(&mut $self.$field), $intent);
    };
}

pub struct App {
    should_quit: bool,
    focus: Focus,
    size: Option<(u16, u16)>,
    /// PTY lifecycle state (MVI pattern).
    pub pty_lifecycle: PtyLifecycleState,
    /// PTY handle (resource, managed outside MVI).
    pty_handle: Option<PtyHandle>,
    config: ConfigStore,
    error_registry: ErrorRegistry,
    ipc_sender: Option<UiCommandSender>,
    proxy_status: Option<ProxyStatus>,
    metrics: Option<MetricsSnapshot>,
    backends: Vec<BackendInfo>,
    backend_selection: usize,
    last_ipc_error: Option<String>,
    last_status_refresh: Instant,
    last_metrics_refresh: Instant,
    last_backends_refresh: Instant,
    /// State of the history dialog (MVI pattern).
    history_dialog: HistoryDialogState,
    /// Provider closure that fetches history entries from backend state.
    history_provider: Option<Arc<dyn Fn() -> Vec<HistoryEntry> + Send + Sync>>,
    /// State of the settings dialog (MVI pattern).
    settings_dialog: SettingsDialogState,
    /// Claude Code settings manager (registry + current values).
    settings_manager: ClaudeSettingsManager,
    /// Snapshot of values when settings dialog was opened (for dirty check).
    settings_saved_snapshot: std::collections::HashMap<crate::config::SettingId, bool>,
    /// Monotonically increasing generation counter. Incremented on each PTY spawn.
    /// Used to tag ProcessExit events and ignore stale exits from old instances.
    pty_generation: u64,
    /// Current mouse text selection (None when nothing is selected).
    selection: Option<TextSelection>,
}

impl App {
    pub fn new(config: ConfigStore) -> Self {
        let now = Instant::now();
        let mut settings_manager = ClaudeSettingsManager::new();
        settings_manager.load_from_toml(&config.get().claude_settings);
        let settings_saved_snapshot = settings_manager.snapshot_values();
        Self {
            should_quit: false,
            focus: Focus::Terminal,
            size: None,
            pty_lifecycle: PtyLifecycleState::default(),
            pty_handle: None,
            config,
            error_registry: ErrorRegistry::new(100),
            ipc_sender: None,
            proxy_status: None,
            metrics: None,
            backends: Vec::new(),
            backend_selection: 0,
            last_ipc_error: None,
            last_status_refresh: now,
            last_metrics_refresh: now,
            last_backends_refresh: now,
            history_dialog: HistoryDialogState::default(),
            history_provider: None,
            settings_dialog: SettingsDialogState::default(),
            settings_manager,
            settings_saved_snapshot,
            pty_generation: 0,
            selection: None,
        }
    }

    /// Get access to the error registry.
    pub fn error_registry(&self) -> &ErrorRegistry {
        &self.error_registry
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    pub fn show_popup(&self) -> bool {
        matches!(self.focus, Focus::Popup(_))
    }

    pub fn popup_kind(&self) -> Option<PopupKind> {
        match self.focus {
            Focus::Popup(kind) => Some(kind),
            Focus::Terminal => None,
        }
    }

    pub fn focus_is_terminal(&self) -> bool {
        self.focus == Focus::Terminal
    }

    /// True once the child process has produced its first output.
    pub fn is_pty_ready(&self) -> bool {
        self.pty_lifecycle.is_ready()
    }

    pub fn toggle_popup(&mut self, kind: PopupKind) -> bool {
        self.focus = match self.focus {
            Focus::Popup(active) if active == kind => Focus::Terminal,
            _ => {
                if kind == PopupKind::BackendSwitch {
                    self.reset_backend_selection();
                }
                Focus::Popup(kind)
            }
        };
        matches!(self.focus, Focus::Popup(_))
    }

    pub fn close_popup(&mut self) {
        self.focus = Focus::Terminal;
    }

    pub fn on_tick(&mut self) {
    }

    /// Send input to PTY or buffer if not ready.
    pub fn send_input(&mut self, bytes: &[u8]) {
        if self.pty_lifecycle.is_ready() {
            if let Some(pty) = &self.pty_handle {
                let _ = pty.send_input(bytes);
            }
        } else {
            self.dispatch_pty(PtyIntent::BufferInput {
                bytes: bytes.to_vec(),
            });
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        let bracketed = format!("\x1b[200~{}\x1b[201~", text);
        self.send_input(bracketed.as_bytes());
    }


    pub fn on_resize(&mut self, cols: u16, rows: u16) {
        self.size = Some((cols, rows));
        if let Some(pty) = &self.pty_handle {
            let _ = pty.resize(cols, rows);
        }
    }

    /// Attach PTY handle. Stores the resource and transitions state.
    pub fn attach_pty(&mut self, pty: PtyHandle) {
        self.pty_handle = Some(pty);
        self.dispatch_pty(PtyIntent::Attach);
    }

    /// Called when PTY produces output.
    ///
    /// Returns `true` if the lifecycle just transitioned to `Ready`.
    ///
    /// Transitions to `Ready` once the child process has both:
    /// 1. Hidden the hardware cursor (DECTCEM off) — UI framework took control
    /// 2. Rendered content (cursor moved past row 0) — first frame is drawn
    ///
    /// React Ink's startup order is: hide cursor → setRawMode → render frame.
    /// By requiring rendered content we guarantee setRawMode has been called,
    /// so the PTY slave no longer echoes input.
    pub fn on_pty_output(&mut self) -> bool {
        if self.pty_lifecycle.is_ready() {
            return false;
        }

        let (cursor_hidden, cursor_row) = self
            .pty_handle
            .as_ref()
            .map(|pty| {
                let c = pty.emulator().lock().cursor();
                (!c.visible, c.row)
            })
            .unwrap_or((false, 0));

        // Wait until cursor is hidden AND child has rendered content.
        if !cursor_hidden || cursor_row == 0 {
            return false;
        }

        // Extract buffer before state transition.
        let buffer = match &mut self.pty_lifecycle {
            PtyLifecycleState::Attached { buffer } => std::mem::take(buffer),
            _ => VecDeque::new(),
        };
        self.dispatch_pty(PtyIntent::GotOutput);
        // Flush buffered input now that child UI is active.
        if let Some(pty) = &self.pty_handle {
            for bytes in buffer {
                let _ = pty.send_input(&bytes);
            }
        }
        true
    }

    pub fn emulator(
        &self,
    ) -> Option<Arc<Mutex<Box<dyn crate::pty::TerminalEmulator>>>> {
        self.pty_handle.as_ref().map(|pty| pty.emulator())
    }

    /// Check if mouse tracking is enabled by the application.
    pub fn mouse_tracking(&self) -> bool {
        self.pty_handle
            .as_ref()
            .map(|pty| pty.emulator().lock().mouse_tracking())
            .unwrap_or(false)
    }

    /// Scroll up (view older content).
    pub fn scroll_up(&mut self, lines: usize) {
        if let Some(pty) = &self.pty_handle {
            pty.scroll_up(lines);
        }
    }

    /// Scroll down (view newer content).
    pub fn scroll_down(&mut self, lines: usize) {
        if let Some(pty) = &self.pty_handle {
            pty.scroll_down(lines);
        }
    }

    /// Reset scrollback to live view.
    pub fn reset_scrollback(&mut self) {
        if let Some(pty) = &self.pty_handle {
            pty.reset_scrollback();
        }
    }

    /// Get current scrollback offset.
    pub fn scrollback(&self) -> usize {
        self.pty_handle.as_ref().map(|pty| pty.scrollback()).unwrap_or(0)
    }

    // ========================================================================
    // Mouse text selection
    // ========================================================================

    /// Current selection state (for rendering).
    pub fn selection(&self) -> Option<&TextSelection> {
        self.selection.as_ref()
    }

    /// Start a new selection at the given grid position.
    pub fn start_selection(&mut self, pos: GridPos) {
        self.selection = Some(TextSelection::new(pos));
    }

    /// Update the selection end position (during drag).
    pub fn update_selection(&mut self, pos: GridPos) {
        if let Some(sel) = &mut self.selection {
            sel.end = pos;
        }
    }

    /// Clear the selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Finalize the selection: mark inactive, extract text from grid.
    /// Returns the selected text, or None if no selection.
    pub fn finish_selection(&mut self) -> Option<String> {
        let sel = self.selection.as_mut()?;
        sel.active = false;
        let text = self
            .pty_handle
            .as_ref()
            .map(|pty| {
                let emu = pty.emulator();
                let guard = emu.lock();
                sel.extract_text(&**guard)
            })?;
        Some(text)
    }

    pub fn set_ipc_sender(&mut self, sender: UiCommandSender) {
        self.ipc_sender = Some(sender);
    }

    pub fn proxy_status(&self) -> Option<&ProxyStatus> {
        self.proxy_status.as_ref()
    }

    pub fn metrics(&self) -> Option<&MetricsSnapshot> {
        self.metrics.as_ref()
    }

    pub fn backends(&self) -> &[BackendInfo] {
        &self.backends
    }

    pub fn backend_selection(&self) -> usize {
        self.backend_selection
    }

    pub fn last_ipc_error(&self) -> Option<&str> {
        self.last_ipc_error.as_deref()
    }

    pub fn update_status(&mut self, status: ProxyStatus) {
        self.proxy_status = Some(status);
    }

    pub fn update_metrics(&mut self, metrics: MetricsSnapshot) {
        self.metrics = Some(metrics);
    }

    pub fn update_backends(&mut self, backends: Vec<BackendInfo>) {
        let was_empty = self.backends.is_empty();
        self.backends = backends;
        if was_empty {
            self.reset_backend_selection();
            return;
        }
        self.clamp_backend_selection();
    }

    pub fn set_ipc_error(&mut self, message: String) {
        self.last_ipc_error = Some(message);
    }

    pub fn clear_ipc_error(&mut self) {
        self.last_ipc_error = None;
    }

    pub fn request_status_refresh(&mut self) {
        self.send_command(UiCommand::RefreshStatus);
    }

    pub fn request_metrics_refresh(&mut self, backend_id: Option<String>) {
        self.send_command(UiCommand::RefreshMetrics { backend_id });
    }

    pub fn request_backends_refresh(&mut self) {
        self.send_command(UiCommand::RefreshBackends);
    }

    pub fn request_config_reload(&mut self) {
        self.send_command(UiCommand::ReloadConfig);
    }

    pub fn request_switch_backend_by_index(&mut self, index: usize) -> bool {
        let Some(backend) = self.backends.get(index.saturating_sub(1)) else {
            return false;
        };
        self.send_command(UiCommand::SwitchBackend {
            backend_id: backend.id.clone(),
        })
    }

    pub fn move_backend_selection(&mut self, direction: i32) {
        if self.backends.is_empty() {
            self.backend_selection = 0;
            return;
        }

        let len = self.backends.len();
        let current = self.backend_selection.min(len.saturating_sub(1));
        let next = if direction.is_negative() {
            if current == 0 {
                len - 1
            } else {
                current - 1
            }
        } else if current + 1 >= len {
            0
        } else {
            current + 1
        };

        self.backend_selection = next;
    }

    pub fn should_refresh_status(&mut self, interval: Duration) -> bool {
        if self.last_status_refresh.elapsed() >= interval {
            self.last_status_refresh = Instant::now();
            return true;
        }
        false
    }

    pub fn should_refresh_metrics(&mut self, interval: Duration) -> bool {
        if self.last_metrics_refresh.elapsed() >= interval {
            self.last_metrics_refresh = Instant::now();
            return true;
        }
        false
    }

    pub fn should_refresh_backends(&mut self, interval: Duration) -> bool {
        if self.last_backends_refresh.elapsed() >= interval {
            self.last_backends_refresh = Instant::now();
            return true;
        }
        false
    }

    /// Called when config file has been reloaded.
    ///
    /// The new config is already available via `self.config.get()`.
    /// This method can update any cached state derived from config.
    pub fn on_config_reload(&mut self) {
        // Currently just a notification point.
        // Future: update cached backend list, theme, etc.
        let _config = self.config.get();
    }

    // ========================================================================
    // PTY lifecycle methods (MVI pattern)
    // ========================================================================

    /// Dispatch an intent to the PTY lifecycle reducer.
    pub fn dispatch_pty(&mut self, intent: PtyIntent) {
        dispatch_mvi!(self, pty_lifecycle, PtyReducer, intent);
    }

    // ========================================================================
    // History dialog methods (MVI pattern)
    // ========================================================================

    /// Set the history provider closure (called from runtime).
    pub fn set_history_provider(
        &mut self,
        provider: Arc<dyn Fn() -> Vec<HistoryEntry> + Send + Sync>,
    ) {
        self.history_provider = Some(provider);
    }

    /// Get the current history dialog state.
    pub fn history_dialog(&self) -> &HistoryDialogState {
        &self.history_dialog
    }

    /// Dispatch an intent to the history dialog reducer.
    pub fn dispatch_history(&mut self, intent: HistoryIntent) {
        dispatch_mvi!(self, history_dialog, HistoryReducer, intent);
    }

    /// Open the history dialog by loading entries from the provider.
    pub fn open_history_dialog(&mut self) {
        let entries = self
            .history_provider
            .as_ref()
            .map(|p| p())
            .unwrap_or_default();
        self.dispatch_history(HistoryIntent::Load { entries });
        self.focus = Focus::Popup(PopupKind::History);
    }

    /// Close the history dialog.
    pub fn close_history_dialog(&mut self) {
        self.dispatch_history(HistoryIntent::Close);
        self.focus = Focus::Terminal;
    }

    // ========================================================================
    // Settings dialog methods (MVI pattern)
    // ========================================================================

    /// Get the current settings dialog state.
    pub fn settings_dialog(&self) -> &SettingsDialogState {
        &self.settings_dialog
    }

    /// Get the settings manager.
    pub fn settings_manager(&self) -> &ClaudeSettingsManager {
        &self.settings_manager
    }

    /// Dispatch an intent to the settings dialog reducer.
    pub fn dispatch_settings(&mut self, intent: SettingsIntent) {
        dispatch_mvi!(self, settings_dialog, SettingsReducer, intent);
    }

    /// Open the settings dialog by loading snapshots from the manager.
    pub fn open_settings_dialog(&mut self) {
        let fields = self.settings_manager.to_snapshots();
        self.settings_saved_snapshot = self.settings_manager.snapshot_values();
        self.dispatch_settings(SettingsIntent::Load { fields });
        self.focus = Focus::Popup(PopupKind::Settings);
    }

    /// Close the settings dialog without applying (unconditional).
    pub fn close_settings_dialog(&mut self) {
        self.dispatch_settings(SettingsIntent::Close);
        self.focus = Focus::Terminal;
    }

    /// Request close: if dirty and not yet confirming, shows warning. Otherwise closes.
    pub fn request_close_settings(&mut self) {
        self.dispatch_settings(SettingsIntent::RequestClose);
        if !self.settings_dialog.is_visible() {
            self.focus = Focus::Terminal;
        }
    }

    /// Current PTY generation counter.
    pub fn pty_generation(&self) -> u64 {
        self.pty_generation
    }

    /// True if at least one PTY restart has occurred during this session.
    pub fn has_restarted(&self) -> bool {
        self.pty_generation > 0
    }

    /// Increment and return the new PTY generation (called before each spawn).
    pub fn next_pty_generation(&mut self) -> u64 {
        self.pty_generation += 1;
        self.pty_generation
    }

    /// Apply settings from the dialog. Returns true if PTY restart was requested.
    pub fn apply_settings(&mut self) -> bool {
        let fields = match &self.settings_dialog {
            SettingsDialogState::Visible { fields, .. } => fields.clone(),
            _ => return false,
        };

        self.settings_manager.apply_snapshots(&fields);

        if !self.settings_manager.is_dirty(&self.settings_saved_snapshot) {
            self.close_settings_dialog();
            return false;
        }

        let env_vars = self.settings_manager.to_env_vars();
        let cli_args = self.settings_manager.to_cli_args();
        let settings_toml = self.settings_manager.to_toml_map();

        self.settings_saved_snapshot = self.settings_manager.snapshot_values();
        self.close_settings_dialog();

        // Transition to Restarting BEFORE sending the command — any ProcessExit
        // from the old PTY that arrives between now and the actual restart will
        // be ignored because the lifecycle state is Restarting.
        self.dispatch_pty(PtyIntent::Detach);

        if !self.send_command(UiCommand::RestartPty {
            env_vars,
            cli_args,
            settings_toml,
        }) {
            // Command failed to send, revert to Pending
            self.dispatch_pty(PtyIntent::SpawnFailed);
            return false;
        }
        true
    }

    /// Detach PTY handle and reset lifecycle for restart.
    pub fn detach_pty(&mut self) {
        self.pty_handle = None;
        self.dispatch_pty(PtyIntent::Detach);
    }

    fn send_command(&mut self, command: UiCommand) -> bool {
        let Some(sender) = &self.ipc_sender else {
            return false;
        };

        match sender.try_send(command) {
            Ok(()) => {
                self.clear_ipc_error();
                true
            }
            Err(err) => {
                self.set_ipc_error(format!("IPC send failed: {}", err));
                false
            }
        }
    }

    fn reset_backend_selection(&mut self) {
        self.backend_selection = self.active_backend_index().unwrap_or(0);
    }

    fn clamp_backend_selection(&mut self) {
        if self.backends.is_empty() {
            self.backend_selection = 0;
            return;
        }
        let max_index = self.backends.len() - 1;
        if self.backend_selection > max_index {
            self.backend_selection = max_index;
        }
    }

    fn active_backend_index(&self) -> Option<usize> {
        self.backends.iter().position(|backend| backend.is_active)
    }
}

