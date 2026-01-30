use crate::ui::app::{App, PopupKind};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if key.kind != KeyEventKind::Press {
        return;
    }

    if is_ctrl_char(key, 'q') {
        app.request_quit();
        return;
    }

    if app.show_popup() {
        if matches!(key.code, KeyCode::Esc) {
            app.close_popup();
            return;
        }
        if is_ctrl_char(key, 'b') {
            let opened = app.toggle_popup(PopupKind::BackendSwitch);
            if opened {
                app.request_backends_refresh();
            }
            return;
        }
        if is_ctrl_char(key, 's') {
            let opened = app.toggle_popup(PopupKind::Status);
            if opened {
                app.request_status_refresh();
                app.request_metrics_refresh(None);
            }
            return;
        }
        if matches!(app.popup_kind(), Some(PopupKind::BackendSwitch)) {
            if let KeyCode::Char(ch) = key.code {
                if ch.is_ascii_digit() {
                    let index = ch.to_digit(10).unwrap_or(0) as usize;
                    if index > 0 && app.request_switch_backend_by_index(index) {
                        app.close_popup();
                    }
                    return;
                }
            }
        }
        return;
    }

    if is_ctrl_char(key, 'b') {
        let opened = app.toggle_popup(PopupKind::BackendSwitch);
        if opened {
            app.request_backends_refresh();
        }
        return;
    }
    if is_ctrl_char(key, 's') {
        let opened = app.toggle_popup(PopupKind::Status);
        if opened {
            app.request_status_refresh();
            app.request_metrics_refresh(None);
        }
        return;
    }

    app.on_key(key);
}

fn is_ctrl_char(key: KeyEvent, needle: char) -> bool {
    matches!(key.code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&needle))
        && key.modifiers.contains(KeyModifiers::CONTROL)
}
