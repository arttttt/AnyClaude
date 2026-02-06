use crate::config::ConfigStore;
use crate::error::ErrorRegistry;
use crate::ipc::{BackendInfo, ProxyStatus};
use crate::metrics::MetricsSnapshot;
use crate::pty::PtyHandle;
use crate::ui::history::{HistoryDialogState, HistoryEntry, HistoryIntent, HistoryReducer};
use crate::ui::mvi::Reducer;
use crate::ui::pty_state::PtyState;
use crate::ui::summarization::{SummarizeDialogState, SummarizeIntent, SummarizeReducer};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PopupKind {
    BackendSwitch,
    Status,
    History,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Terminal,
    Popup(PopupKind),
}

#[derive(Debug)]
pub enum UiCommand {
    SwitchBackend { backend_id: String },
    /// Summarize session and then switch backend.
    SummarizeAndSwitchBackend {
        from_backend: String,
        to_backend: String,
    },
    RefreshStatus,
    RefreshMetrics { backend_id: Option<String> },
    RefreshBackends,
    ReloadConfig,
}

pub type UiCommandSender = mpsc::Sender<UiCommand>;

pub struct App {
    should_quit: bool,
    tick_rate: Duration,
    last_tick: Instant,
    focus: Focus,
    status_message: Option<String>,
    size: Option<(u16, u16)>,
    pty_state: PtyState,
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
    /// State of the summarization dialog (MVI pattern).
    summarize_dialog: SummarizeDialogState,
    /// State of the history dialog (MVI pattern).
    history_dialog: HistoryDialogState,
    /// Provider closure that fetches history entries from backend state.
    history_provider: Option<Arc<dyn Fn() -> Vec<HistoryEntry> + Send + Sync>>,
    /// Backend ID pending switch (waiting for summarization).
    pending_backend_switch: Option<String>,
    /// Selected button in failed state (0 = Retry, 1 = Cancel).
    summarize_button_selection: u8,
    /// Last animation tick for spinner.
    last_animation_tick: Instant,
    /// Scheduled time for next auto-retry (exponential backoff).
    scheduled_retry_at: Option<Instant>,
}

impl App {
    pub fn new(tick_rate: Duration, config: ConfigStore) -> Self {
        let now = Instant::now();
        Self {
            should_quit: false,
            tick_rate,
            last_tick: Instant::now(),
            focus: Focus::Terminal,
            status_message: None,
            size: None,
            pty_state: PtyState::default(),
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
            summarize_dialog: SummarizeDialogState::default(),
            history_dialog: HistoryDialogState::default(),
            history_provider: None,
            pending_backend_switch: None,
            summarize_button_selection: 0,
            last_animation_tick: now,
            scheduled_retry_at: None,
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
        self.last_tick = Instant::now();
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let Some(bytes) = key_event_to_bytes(key) else {
            return;
        };
        self.send_input(&bytes);
    }

    /// Send input to PTY or buffer if not ready.
    fn send_input(&mut self, bytes: &[u8]) {
        self.pty_state.send_input(bytes);
    }

    pub fn on_paste(&mut self, text: &str) {
        // Send paste content wrapped in bracketed paste escape sequences
        // so the subprocess knows this is pasted content
        let bracketed = format!("\x1b[200~{}\x1b[201~", text);
        self.send_input(bracketed.as_bytes());
    }

    pub fn on_image_paste(&mut self, data_uri: &str) {
        // Send image data URI as text input
        let bracketed = format!("\x1b[200~{}\x1b[201~", data_uri);
        self.send_input(bracketed.as_bytes());
    }

    pub fn on_resize(&mut self, cols: u16, rows: u16) {
        self.size = Some((cols, rows));
        if let Some(pty) = self.pty_state.pty_handle() {
            let _ = pty.resize(cols, rows);
        }
    }

    /// Attach PTY handle. Transitions from Pending to Attached state.
    pub fn attach_pty(&mut self, pty: PtyHandle) {
        self.pty_state.attach(pty);
    }

    /// Called when PTY produces output. Transitions to Ready and flushes buffer.
    pub fn on_pty_output(&mut self) {
        self.pty_state.on_output();
    }

    pub fn parser(&self) -> Option<Arc<Mutex<vt100::Parser>>> {
        self.pty_state.pty_handle().map(|pty| pty.parser())
    }

    /// Scroll up (view older content).
    pub fn scroll_up(&mut self, lines: usize) {
        if let Some(pty) = self.pty_state.pty_handle() {
            pty.scroll_up(lines);
        }
    }

    /// Scroll down (view newer content).
    pub fn scroll_down(&mut self, lines: usize) {
        if let Some(pty) = self.pty_state.pty_handle() {
            pty.scroll_down(lines);
        }
    }

    /// Reset scrollback to live view.
    pub fn reset_scrollback(&mut self) {
        if let Some(pty) = self.pty_state.pty_handle() {
            pty.reset_scrollback();
        }
    }

    /// Get current scrollback offset.
    pub fn scrollback(&self) -> usize {
        self.pty_state.pty_handle().map(|pty| pty.scrollback()).unwrap_or(0)
    }

    #[allow(dead_code)]
    pub fn tick_rate(&self) -> Duration {
        self.tick_rate
    }

    #[allow(dead_code)]
    pub fn last_tick(&self) -> Instant {
        self.last_tick
    }

    #[allow(dead_code)]
    pub fn status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
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

    /// Request summarization and backend switch.
    ///
    /// Returns the target backend_id if command was sent successfully.
    pub fn request_summarize_and_switch(&mut self, to_backend_id: String) -> Option<String> {
        let from_backend = self.proxy_status.as_ref()
            .map(|s| s.active_backend.clone())
            .unwrap_or_default();

        if self.send_command(UiCommand::SummarizeAndSwitchBackend {
            from_backend,
            to_backend: to_backend_id.clone(),
        }) {
            Some(to_backend_id)
        } else {
            None
        }
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

    /// Get access to the config store for reading current config.
    #[allow(dead_code)]
    pub fn config(&self) -> &ConfigStore {
        &self.config
    }

    // ========================================================================
    // Summarization dialog methods (MVI pattern)
    // ========================================================================

    /// Get the current summarization dialog state.
    pub fn summarize_dialog(&self) -> &SummarizeDialogState {
        &self.summarize_dialog
    }

    /// Check if summarization dialog is visible.
    pub fn is_summarize_dialog_visible(&self) -> bool {
        !matches!(self.summarize_dialog, SummarizeDialogState::Hidden)
    }

    /// Dispatch an intent to the summarization dialog reducer.
    pub fn dispatch_summarize(&mut self, intent: SummarizeIntent) {
        self.summarize_dialog = SummarizeReducer::reduce(
            std::mem::take(&mut self.summarize_dialog),
            intent,
        );
    }

    /// Start summarization for a backend switch.
    pub fn start_summarization_for_switch(&mut self, backend_id: String) {
        self.pending_backend_switch = Some(backend_id);
        self.dispatch_summarize(SummarizeIntent::Start);
        self.last_animation_tick = Instant::now();
    }

    /// Get the backend ID waiting for summarization.
    pub fn pending_backend_switch(&self) -> Option<&str> {
        self.pending_backend_switch.as_deref()
    }

    /// Complete the pending backend switch after successful summarization.
    pub fn complete_summarization(&mut self) -> Option<String> {
        self.dispatch_summarize(SummarizeIntent::Hide);
        self.pending_backend_switch.take()
    }

    /// Cancel the pending backend switch.
    pub fn cancel_summarization(&mut self) {
        self.dispatch_summarize(SummarizeIntent::Hide);
        self.pending_backend_switch = None;
        self.scheduled_retry_at = None;
    }

    /// Get the currently selected button (0 = Retry, 1 = Cancel).
    pub fn summarize_button_selection(&self) -> u8 {
        self.summarize_button_selection
    }

    /// Toggle button selection.
    pub fn toggle_summarize_button(&mut self) {
        self.summarize_button_selection = if self.summarize_button_selection == 0 { 1 } else { 0 };
    }

    /// Reset button selection to default (Retry).
    pub fn reset_summarize_button(&mut self) {
        self.summarize_button_selection = 0;
    }

    /// Check and update animation tick. Returns true if tick should be sent.
    pub fn should_animate(&mut self, interval: Duration) -> bool {
        if self.is_summarize_dialog_visible() && self.last_animation_tick.elapsed() >= interval {
            self.last_animation_tick = Instant::now();
            return true;
        }
        false
    }

    /// Schedule a retry with exponential backoff.
    /// Delay: 1s for attempt 1, 2s for attempt 2, 4s for attempt 3.
    pub fn schedule_retry(&mut self, attempt: u8) {
        let delay_secs = 1u64 << (attempt.saturating_sub(1).min(4)); // 1, 2, 4, 8, 16 max
        self.scheduled_retry_at = Some(Instant::now() + Duration::from_secs(delay_secs));
    }

    /// Clear any scheduled retry.
    pub fn clear_scheduled_retry(&mut self) {
        self.scheduled_retry_at = None;
    }

    /// Check if a scheduled retry is due. Returns true and clears if due.
    pub fn is_retry_due(&mut self) -> bool {
        if let Some(scheduled) = self.scheduled_retry_at {
            if Instant::now() >= scheduled {
                self.scheduled_retry_at = None;
                return true;
            }
        }
        false
    }

    /// Get seconds remaining until scheduled retry (for UI display).
    pub fn retry_countdown_secs(&self) -> Option<u64> {
        self.scheduled_retry_at.map(|scheduled| {
            scheduled.saturating_duration_since(Instant::now()).as_secs()
        })
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
        self.history_dialog = HistoryReducer::reduce(
            std::mem::take(&mut self.history_dialog),
            intent,
        );
    }

    /// Open the history dialog by loading entries from the provider.
    pub fn open_history_dialog(&mut self) {
        let entries = self
            .history_provider
            .as_ref()
            .map(|p| p())
            .unwrap_or_default();
        self.dispatch_history(HistoryIntent::Load { entries });
    }

    /// Close the history dialog.
    pub fn close_history_dialog(&mut self) {
        self.dispatch_history(HistoryIntent::Close);
    }

    fn send_command(&mut self, command: UiCommand) -> bool {
        let Some(sender) = &self.ipc_sender else {
            self.status_message = Some("IPC not initialized".to_string());
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

fn key_event_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    if key.kind != KeyEventKind::Press {
        return None;
    }

    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let value = (c as u8).to_ascii_lowercase();
                return Some(vec![value.saturating_sub(b'a') + 1]);
            }
            let mut buffer = [0u8; 4];
            Some(c.encode_utf8(&mut buffer).as_bytes().to_vec())
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        _ => None,
    }
}
