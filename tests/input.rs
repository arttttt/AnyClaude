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

#[test]
fn app_shortcut_requires_super() {
    // No modifiers → nothing, even for a bound key.
    assert_eq!(app_shortcut(KeyCode::KeyC, ModifiersState::empty()), None);
    assert_eq!(app_shortcut(KeyCode::KeyC, SUPER), Some(AppShortcut::CopySelection));
}

#[test]
fn app_shortcut_table() {
    assert_eq!(app_shortcut(KeyCode::KeyC, SUPER), Some(AppShortcut::CopySelection));
    assert_eq!(app_shortcut(KeyCode::KeyV, SUPER), Some(AppShortcut::Paste));
    assert_eq!(app_shortcut(KeyCode::KeyB, SUPER), Some(AppShortcut::ToggleBackendPopup));
    assert_eq!(app_shortcut(KeyCode::KeyH, SUPER), Some(AppShortcut::ToggleHistoryPopup));
    assert_eq!(app_shortcut(KeyCode::KeyE, SUPER), Some(AppShortcut::ToggleSettingsPopup));
    assert_eq!(app_shortcut(KeyCode::KeyR, SUPER), Some(AppShortcut::RestartPty));
    assert_eq!(app_shortcut(KeyCode::KeyQ, SUPER), Some(AppShortcut::Quit));
    // Unbound super-combo.
    assert_eq!(app_shortcut(KeyCode::KeyA, SUPER), None);
}

#[test]
fn diagnostic_needs_super_and_shift() {
    // Cmd+D alone is unbound; Cmd+Shift+D dumps.
    assert_eq!(app_shortcut(KeyCode::KeyD, SUPER), None);
    assert_eq!(
        app_shortcut(KeyCode::KeyD, ModifiersState::SUPER | ModifiersState::SHIFT),
        Some(AppShortcut::DumpDiagnostic)
    );
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
