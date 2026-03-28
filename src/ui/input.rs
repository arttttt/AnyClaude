use crate::ui::app::{App, PopupKind};
use crate::ui::backend_switch::{BackendPopupSection, BackendSwitchIntent, BackendSwitchState};
use crate::ui::history::HistoryIntent;
use crate::ui::settings::SettingsIntent;
use term_input::{Direction, KeyInput, KeyKind};

/// Action to take after processing a key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    /// No further action needed (handled internally).
    None,
    /// Forward raw bytes to PTY.
    Forward,
}

/// Classify a key input: hotkey, popup navigation, or forward to PTY.
pub fn classify_key(app: &mut App, key: &KeyInput) -> InputAction {
    // Global hotkeys (regardless of popup state)
    match &key.kind {
        KeyKind::Control('q') => {
            app.request_quit();
            return InputAction::None;
        }
        KeyKind::Control('v') => {
            // Forward to CC — it handles clipboard images natively via
            // osascript on macOS (reads «class PNGf» / «class furl»
            // from the pasteboard).
            return InputAction::Forward;
        }
        _ => {}
    }

    // Popup-specific handling
    if app.show_popup() {
        return handle_popup_key(app, key);
    }

    // Non-popup hotkeys
    match &key.kind {
        KeyKind::Control('b') => {
            let opened = app.toggle_popup(PopupKind::BackendSwitch);
            if opened {
                app.request_backends_refresh();
            }
            InputAction::None
        }
        KeyKind::Control('h') => {
            app.open_history_dialog();
            InputAction::None
        }
        KeyKind::Control('e') => {
            app.open_settings_dialog();
            InputAction::None
        }
        KeyKind::Control('r') => {
            app.request_restart_claude();
            InputAction::None
        }
        _ => InputAction::Forward,
    }
}

/// Handle key input when a popup is open.
fn handle_popup_key(app: &mut App, key: &KeyInput) -> InputAction {
    let popup = match app.popup_kind() {
        Some(kind) => kind,
        None => return InputAction::None,
    };

    match popup {
        PopupKind::History => handle_history_key(app, key),
        PopupKind::Settings => handle_settings_key(app, key),
        PopupKind::BackendSwitch => handle_backend_switch_key(app, key),
    }
}

fn handle_history_key(app: &mut App, key: &KeyInput) -> InputAction {
    match &key.kind {
        KeyKind::Escape | KeyKind::Control('h') => {
            app.close_history_dialog();
        }
        KeyKind::Arrow(Direction::Up) => {
            app.dispatch_history(HistoryIntent::ScrollUp);
        }
        KeyKind::Arrow(Direction::Down) => {
            app.dispatch_history(HistoryIntent::ScrollDown);
        }
        _ => {}
    }
    InputAction::None
}

fn handle_settings_key(app: &mut App, key: &KeyInput) -> InputAction {
    match &key.kind {
        KeyKind::Escape => {
            app.request_close_settings();
        }
        KeyKind::Arrow(Direction::Up) => {
            app.dispatch_settings(SettingsIntent::MoveUp);
        }
        KeyKind::Arrow(Direction::Down) => {
            app.dispatch_settings(SettingsIntent::MoveDown);
        }
        KeyKind::Char(' ') => {
            app.dispatch_settings(SettingsIntent::Toggle);
        }
        KeyKind::Enter => {
            app.apply_settings();
        }
        KeyKind::Control('e') => {
            app.close_settings_dialog();
        }
        _ => {}
    }
    InputAction::None
}

fn handle_backend_switch_key(app: &mut App, key: &KeyInput) -> InputAction {
    match &key.kind {
        KeyKind::Escape | KeyKind::Control('b') | KeyKind::Control('h') => {
            app.close_backend_switch_dialog();
        }
        KeyKind::Tab => {
            app.dispatch_backend_switch(BackendSwitchIntent::NextSection);
        }
        KeyKind::Arrow(Direction::Up) => {
            app.dispatch_backend_switch(BackendSwitchIntent::MoveUp);
        }
        KeyKind::Arrow(Direction::Down) => {
            app.dispatch_backend_switch(BackendSwitchIntent::MoveDown);
        }
        KeyKind::Enter => {
            return handle_backend_confirm(app);
        }
        KeyKind::Backspace | KeyKind::Nav(term_input::NavKey::Delete) => {
            return handle_backend_delete(app);
        }
        KeyKind::Char(ch) if ch.is_ascii_digit() => {
            let index = ch.to_digit(10).unwrap_or(0) as usize;
            if index > 0 {
                return handle_backend_digit(app, index);
            }
        }
        _ => {}
    }
    InputAction::None
}

/// Handle Enter in the currently active section of the backend switch dialog.
fn handle_backend_confirm(app: &mut App) -> InputAction {
    let BackendSwitchState::Visible {
        section,
        backend_selection,
        subagent_selection,
        teammate_selection,
        ..
    } = *app.backend_switch()
    else {
        return InputAction::None;
    };

    match section {
        BackendPopupSection::ActiveBackend => {
            app.confirm_active_backend(backend_selection);
        }
        BackendPopupSection::SubagentBackend => {
            app.confirm_override_backend(
                subagent_selection,
                App::request_set_subagent_backend,
                App::request_clear_subagent_backend,
            );
        }
        BackendPopupSection::TeammateBackend => {
            app.confirm_override_backend(
                teammate_selection,
                App::request_set_teammate_backend,
                App::request_clear_teammate_backend,
            );
        }
    }
    InputAction::None
}

/// Handle Backspace/Delete — clear override in subagent/teammate sections.
fn handle_backend_delete(app: &mut App) -> InputAction {
    let BackendSwitchState::Visible { section, .. } = *app.backend_switch() else {
        return InputAction::None;
    };

    match section {
        BackendPopupSection::SubagentBackend => {
            app.request_clear_subagent_backend();
            app.close_backend_switch_dialog();
        }
        BackendPopupSection::TeammateBackend => {
            app.request_clear_teammate_backend();
            app.close_backend_switch_dialog();
        }
        BackendPopupSection::ActiveBackend => {}
    }
    InputAction::None
}

/// Handle digit keys for quick selection.
fn handle_backend_digit(app: &mut App, index: usize) -> InputAction {
    let BackendSwitchState::Visible { section, .. } = *app.backend_switch() else {
        return InputAction::None;
    };

    match section {
        BackendPopupSection::ActiveBackend => {
            let Some(backend) = app.backends().get(index.saturating_sub(1)) else {
                return InputAction::None;
            };
            if backend.is_active {
                app.close_backend_switch_dialog();
                return InputAction::None;
            }
            if app.request_switch_backend_by_index(index) {
                app.close_backend_switch_dialog();
            }
        }
        BackendPopupSection::SubagentBackend => {
            if index <= app.backends().len() {
                app.request_set_subagent_backend(index - 1);
                app.close_backend_switch_dialog();
            }
        }
        BackendPopupSection::TeammateBackend => {
            if index <= app.backends().len() {
                app.request_set_teammate_backend(index - 1);
                app.close_backend_switch_dialog();
            }
        }
    }
    InputAction::None
}
