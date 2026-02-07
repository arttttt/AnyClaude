use crate::config::ThinkingMode;
use crate::ui::app::{App, PopupKind};
use crate::ui::history::HistoryIntent;
use crate::ui::summarization::SummarizeDialogState;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Action to take after processing a key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    /// No further action needed (handled internally).
    None,
    /// Request image paste from clipboard.
    ImagePaste,
    /// Retry summarization (used when in Failed state).
    RetrySummarization,
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> InputAction {
    if key.kind != KeyEventKind::Press {
        return InputAction::None;
    }

    if is_ctrl_char(key, 'q') {
        app.request_quit();
        return InputAction::None;
    }

    // Ctrl+V / Ctrl+Shift+V / Cmd+V: paste with image support
    // We intercept because terminal can't represent image content as text.
    // When clipboard has an image, terminal either sends empty paste or key event.
    if is_ctrl_char(key, 'v') || is_ctrl_shift_char(key, 'v') || is_super_char(key, 'v') {
        return InputAction::ImagePaste;
    }

    if app.show_popup() {
        // Handle summarization dialog when visible (has priority)
        if app.is_summarize_dialog_visible() {
            return handle_summarize_dialog_key(app, key);
        }

        // Handle history dialog keys
        if matches!(app.popup_kind(), Some(PopupKind::History)) {
            match key.code {
                KeyCode::Esc => {
                    app.close_history_dialog();
                    app.close_popup();
                    return InputAction::None;
                }
                KeyCode::Up => {
                    app.dispatch_history(HistoryIntent::ScrollUp);
                    return InputAction::None;
                }
                KeyCode::Down => {
                    app.dispatch_history(HistoryIntent::ScrollDown);
                    return InputAction::None;
                }
                _ => {}
            }
            if is_ctrl_char(key, 'h') {
                app.close_history_dialog();
                app.close_popup();
                return InputAction::None;
            }
            return InputAction::None;
        }

        if matches!(key.code, KeyCode::Esc) {
            app.close_popup();
            return InputAction::None;
        }
        if is_ctrl_char(key, 'b') {
            let opened = app.toggle_popup(PopupKind::BackendSwitch);
            if opened {
                app.request_backends_refresh();
            }
            return InputAction::None;
        }
        if is_ctrl_char(key, 's') {
            let opened = app.toggle_popup(PopupKind::Status);
            if opened {
                app.request_status_refresh();
                app.request_metrics_refresh(None);
            }
            return InputAction::None;
        }
        if is_ctrl_char(key, 'h') {
            app.close_popup();
            return InputAction::None;
        }
        if matches!(app.popup_kind(), Some(PopupKind::BackendSwitch)) {
            match key.code {
                KeyCode::Up => {
                    app.move_backend_selection(-1);
                    return InputAction::None;
                }
                KeyCode::Down => {
                    app.move_backend_selection(1);
                    return InputAction::None;
                }
                KeyCode::Enter => {
                    return handle_backend_switch_enter(app);
                }
                _ => {}
            }
            if let KeyCode::Char(ch) = key.code {
                if ch.is_ascii_digit() {
                    let index = ch.to_digit(10).unwrap_or(0) as usize;
                    if index > 0 {
                        return handle_backend_switch_by_number(app, index);
                    }
                    return InputAction::None;
                }
            }
        }
        return InputAction::None;
    }

    if is_ctrl_char(key, 'b') {
        let opened = app.toggle_popup(PopupKind::BackendSwitch);
        if opened {
            app.request_backends_refresh();
        }
        return InputAction::None;
    }
    if is_ctrl_char(key, 's') {
        let opened = app.toggle_popup(PopupKind::Status);
        if opened {
            app.request_status_refresh();
            app.request_metrics_refresh(None);
        }
        return InputAction::None;
    }
    if is_ctrl_char(key, 'h') {
        app.open_history_dialog();
        app.toggle_popup(PopupKind::History);
        return InputAction::None;
    }

    app.on_key(key);
    InputAction::None
}

fn is_ctrl_char(key: KeyEvent, needle: char) -> bool {
    matches!(key.code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&needle))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::SHIFT)
}

fn is_ctrl_shift_char(key: KeyEvent, needle: char) -> bool {
    matches!(key.code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&needle))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.modifiers.contains(KeyModifiers::SHIFT)
}

fn is_super_char(key: KeyEvent, needle: char) -> bool {
    matches!(key.code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&needle))
        && key.modifiers.contains(KeyModifiers::SUPER)
}

/// Handle Enter key in backend switch popup.
fn handle_backend_switch_enter(app: &mut App) -> InputAction {
    let index = app.backend_selection();
    let backends = app.backends();

    let Some(backend) = backends.get(index) else {
        return InputAction::None;
    };

    // If already active, just close
    if backend.is_active {
        app.close_popup();
        return InputAction::None;
    }

    // Check if summarize mode is enabled
    let config = app.config().get();
    if config.thinking.mode == ThinkingMode::Summarize {
        let backend_id = backend.id.clone();
        // Start summarization flow
        if let Some(id) = app.request_summarize_and_switch(backend_id.clone()) {
            app.start_summarization_for_switch(id);
        }
        return InputAction::None;
    }

    // Normal mode: switch immediately
    if app.request_switch_backend_by_index(index + 1) {
        app.close_popup();
    }
    InputAction::None
}

/// Handle number key in backend switch popup.
fn handle_backend_switch_by_number(app: &mut App, index: usize) -> InputAction {
    let backends = app.backends();

    let Some(backend) = backends.get(index.saturating_sub(1)) else {
        return InputAction::None;
    };

    if backend.is_active {
        app.close_popup();
        return InputAction::None;
    }

    let config = app.config().get();
    if config.thinking.mode == ThinkingMode::Summarize {
        let backend_id = backend.id.clone();
        if let Some(id) = app.request_summarize_and_switch(backend_id.clone()) {
            app.start_summarization_for_switch(id);
        }
        return InputAction::None;
    }

    if app.request_switch_backend_by_index(index) {
        app.close_popup();
    }
    InputAction::None
}

/// Handle key events when summarization dialog is visible.
fn handle_summarize_dialog_key(app: &mut App, key: KeyEvent) -> InputAction {
    match app.summarize_dialog() {
        // In progress: only Esc cancels
        SummarizeDialogState::Summarizing { .. } | SummarizeDialogState::Retrying { .. } => {
            if matches!(key.code, KeyCode::Esc) {
                app.cancel_summarization();
                return InputAction::None;
            }
        }

        // Failed: Tab toggles buttons, Enter selects, Esc/R/C shortcuts
        SummarizeDialogState::Failed { .. } => {
            match key.code {
                KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                    app.toggle_summarize_button();
                }
                KeyCode::Enter => {
                    if app.summarize_button_selection() == 0 {
                        app.reset_summarize_button();
                        return InputAction::RetrySummarization;
                    } else {
                        app.cancel_summarization();
                        return InputAction::None;
                    }
                }
                KeyCode::Esc => {
                    app.cancel_summarization();
                    return InputAction::None;
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    app.reset_summarize_button();
                    return InputAction::RetrySummarization;
                }
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    app.cancel_summarization();
                    return InputAction::None;
                }
                _ => {}
            }
        }

        SummarizeDialogState::Hidden => {}
    }

    InputAction::None
}
