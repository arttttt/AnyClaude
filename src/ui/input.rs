use crate::ui::app::{App, BackendPopupSection, PopupKind};
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
            app.close_popup();
        }
        KeyKind::Tab => {
            app.toggle_backend_popup_section();
        }
        KeyKind::Arrow(Direction::Up) => {
            match app.backend_popup_section() {
                BackendPopupSection::ActiveBackend => app.move_backend_selection(-1),
                BackendPopupSection::SubagentBackend => app.move_subagent_selection(-1),
                BackendPopupSection::TeammateBackend => app.move_teammate_selection(-1),
            }
        }
        KeyKind::Arrow(Direction::Down) => {
            match app.backend_popup_section() {
                BackendPopupSection::ActiveBackend => app.move_backend_selection(1),
                BackendPopupSection::SubagentBackend => app.move_subagent_selection(1),
                BackendPopupSection::TeammateBackend => app.move_teammate_selection(1),
            }
        }
        KeyKind::Enter => {
            match app.backend_popup_section() {
                BackendPopupSection::ActiveBackend => return handle_backend_switch_enter(app),
                BackendPopupSection::SubagentBackend => return handle_subagent_backend_enter(app),
                BackendPopupSection::TeammateBackend => return handle_teammate_backend_enter(app),
            }
        }
        KeyKind::Backspace | KeyKind::Nav(term_input::NavKey::Delete) => {
            match app.backend_popup_section() {
                BackendPopupSection::SubagentBackend => {
                    app.request_clear_subagent_backend();
                    app.close_popup();
                }
                BackendPopupSection::TeammateBackend => {
                    app.request_clear_teammate_backend();
                    app.close_popup();
                }
                _ => {}
            }
        }
        KeyKind::Char(ch) if ch.is_ascii_digit() => {
            let index = ch.to_digit(10).unwrap_or(0) as usize;
            if index > 0 {
                match app.backend_popup_section() {
                    BackendPopupSection::ActiveBackend => return handle_backend_switch_by_number(app, index),
                    BackendPopupSection::SubagentBackend => {
                        if index <= app.backends().len() {
                            app.request_set_subagent_backend(index - 1);
                            app.close_popup();
                        }
                        return InputAction::None;
                    }
                    BackendPopupSection::TeammateBackend => {
                        if index <= app.backends().len() {
                            app.request_set_teammate_backend(index - 1);
                            app.close_popup();
                        }
                        return InputAction::None;
                    }
                }
            }
        }
        _ => {}
    }
    InputAction::None
}

/// Handle Enter key in backend switch popup.
fn handle_backend_switch_enter(app: &mut App) -> InputAction {
    let index = app.backend_selection();
    let backends = app.backends();

    let Some(backend) = backends.get(index) else {
        return InputAction::None;
    };

    if backend.is_active {
        app.close_popup();
        return InputAction::None;
    }

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

    if app.request_switch_backend_by_index(index) {
        app.close_popup();
    }
    InputAction::None
}

/// Handle Enter key in subagent backend section.
fn handle_subagent_backend_enter(app: &mut App) -> InputAction {
    let sel = app.subagent_selection();
    if sel == 0 {
        // "Disabled" — clear subagent backend
        app.request_clear_subagent_backend();
        app.close_popup();
    } else {
        // Backend at index sel-1
        let backend_index = sel - 1;
        if backend_index < app.backends().len() {
            app.request_set_subagent_backend(backend_index);
            app.close_popup();
        }
    }
    InputAction::None
}

fn handle_teammate_backend_enter(app: &mut App) -> InputAction {
    let sel = app.teammate_selection();
    if sel == 0 {
        // "Disabled" — clear teammate backend
        app.request_clear_teammate_backend();
        app.close_popup();
    } else {
        let backend_index = sel - 1;
        if backend_index < app.backends().len() {
            app.request_set_teammate_backend(backend_index);
            app.close_popup();
        }
    }
    InputAction::None
}
