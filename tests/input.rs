//! Pure input → intent mapping (Phase E.3, the R7 event phase). Pins the
//! shortcut table + popup navigation so the coordinator's key routing is
//! tested without a window. (Intent enums aren't PartialEq, so they're matched
//! with `matches!`; AppShortcut is Eq.)

use anyclaude::ui::backend_switch::BackendSwitchIntent;
use anyclaude::ui::history::HistoryIntent;
use anyclaude::ui::input::{app_shortcut, backend_switch_nav, history_nav, settings_nav, AppShortcut};
use anyclaude::ui::settings::SettingsIntent;
use winit::keyboard::{KeyCode, ModifiersState};

const SUPER: ModifiersState = ModifiersState::SUPER;
const CTRL: ModifiersState = ModifiersState::CONTROL;

#[test]
fn clipboard_is_on_cmd_not_ctrl() {
    // No modifiers → nothing, even for a bound key.
    assert_eq!(app_shortcut(KeyCode::KeyC, ModifiersState::empty()), None);
    // Copy / paste are Cmd (Ctrl+C/V are interrupt / literal-next).
    assert_eq!(app_shortcut(KeyCode::KeyC, SUPER), Some(AppShortcut::CopySelection));
    assert_eq!(app_shortcut(KeyCode::KeyV, SUPER), Some(AppShortcut::Paste));
    assert_eq!(app_shortcut(KeyCode::KeyC, CTRL), None);
}

#[test]
fn features_are_on_ctrl() {
    assert_eq!(app_shortcut(KeyCode::KeyT, CTRL), Some(AppShortcut::ToggleBackendPopup));
    assert_eq!(app_shortcut(KeyCode::KeyH, CTRL), Some(AppShortcut::ToggleHistoryPopup));
    assert_eq!(app_shortcut(KeyCode::KeyE, CTRL), Some(AppShortcut::ToggleSettingsPopup));
    assert_eq!(app_shortcut(KeyCode::KeyR, CTRL), Some(AppShortcut::RestartPty));
    assert_eq!(app_shortcut(KeyCode::KeyQ, CTRL), Some(AppShortcut::Quit));
    // Features are not on Cmd.
    assert_eq!(app_shortcut(KeyCode::KeyT, SUPER), None);
    // Unbound Ctrl combo.
    assert_eq!(app_shortcut(KeyCode::KeyA, CTRL), None);
}

#[test]
fn ctrl_b_and_ctrl_d_pass_through_to_the_terminal() {
    // Ctrl+B is Claude Code's; Ctrl+D is EOF — neither is an app shortcut, so
    // they fall through to `encode_key` → the PTY.
    assert_eq!(app_shortcut(KeyCode::KeyB, CTRL), None);
    assert_eq!(app_shortcut(KeyCode::KeyD, CTRL), None);
}

#[cfg(debug_assertions)]
#[test]
fn diagnostic_on_ctrl_g_debug_only() {
    // The diagnostic dump is a debug-build-only dev aid.
    assert_eq!(app_shortcut(KeyCode::KeyG, CTRL), Some(AppShortcut::DumpDiagnostic));
}

#[test]
fn backend_switch_navigation() {
    assert!(matches!(backend_switch_nav(KeyCode::ArrowUp), Some(BackendSwitchIntent::MoveUp)));
    assert!(matches!(backend_switch_nav(KeyCode::ArrowDown), Some(BackendSwitchIntent::MoveDown)));
    assert!(matches!(backend_switch_nav(KeyCode::Tab), Some(BackendSwitchIntent::NextSection)));
    assert!(matches!(backend_switch_nav(KeyCode::Delete), Some(BackendSwitchIntent::Clear)));
    assert!(matches!(backend_switch_nav(KeyCode::Backspace), Some(BackendSwitchIntent::Clear)));
    // Enter is NOT navigation (it applies + closes — the caller's effect).
    assert!(backend_switch_nav(KeyCode::Enter).is_none());
    assert!(backend_switch_nav(KeyCode::KeyA).is_none());
}

#[test]
fn history_navigation() {
    assert!(matches!(history_nav(KeyCode::ArrowUp), Some(HistoryIntent::ScrollUp)));
    assert!(matches!(history_nav(KeyCode::ArrowDown), Some(HistoryIntent::ScrollDown)));
    assert!(history_nav(KeyCode::Enter).is_none());
}

#[test]
fn settings_navigation() {
    assert!(matches!(settings_nav(KeyCode::ArrowUp), Some(SettingsIntent::MoveUp)));
    assert!(matches!(settings_nav(KeyCode::ArrowDown), Some(SettingsIntent::MoveDown)));
    assert!(matches!(settings_nav(KeyCode::Space), Some(SettingsIntent::Toggle)));
    assert!(settings_nav(KeyCode::Enter).is_none());
}
