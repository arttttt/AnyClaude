//! Pure input → intent mapping — the R7 "event phase". Raw winit keys become
//! the app / popup intent vocabulary; no `&self`, no effects. The coordinator
//! (today `GpuApp`) performs the resulting action. Matching is on the PHYSICAL
//! key so shortcuts work on every keyboard layout (Cmd+C must hit on a
//! Russian / Greek layout where the logical key is `с` / `ψ`). Unit-tested
//! without a window.

use winit::keyboard::{KeyCode, ModifiersState};

use crate::ui::backend_switch::BackendSwitchIntent;
use crate::ui::history::HistoryIntent;
use crate::ui::settings::SettingsIntent;

/// App-level (Cmd / Super) shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppShortcut {
    CopySelection,
    Paste,
    ToggleBackendPopup,
    ToggleHistoryPopup,
    ToggleSettingsPopup,
    RestartPty,
    DumpDiagnostic,
    Quit,
}

/// Map a Super-combo to its app shortcut. `None` when Super isn't held or the
/// key isn't bound.
pub fn app_shortcut(code: KeyCode, modifiers: ModifiersState) -> Option<AppShortcut> {
    if !modifiers.super_key() {
        return None;
    }
    Some(match code {
        KeyCode::KeyC => AppShortcut::CopySelection,
        KeyCode::KeyV => AppShortcut::Paste,
        KeyCode::KeyB => AppShortcut::ToggleBackendPopup,
        KeyCode::KeyH => AppShortcut::ToggleHistoryPopup,
        KeyCode::KeyE => AppShortcut::ToggleSettingsPopup,
        KeyCode::KeyR => AppShortcut::RestartPty,
        KeyCode::KeyD if modifiers.shift_key() => AppShortcut::DumpDiagnostic,
        KeyCode::KeyQ => AppShortcut::Quit,
        _ => return None,
    })
}

/// Backend-switch popup navigation. `Enter` is intentionally absent — it
/// applies the selection and closes the popup (an effect the caller performs).
pub fn backend_switch_nav(code: KeyCode) -> Option<BackendSwitchIntent> {
    match code {
        KeyCode::ArrowUp => Some(BackendSwitchIntent::MoveUp),
        KeyCode::ArrowDown => Some(BackendSwitchIntent::MoveDown),
        KeyCode::Tab => Some(BackendSwitchIntent::NextSection),
        KeyCode::Delete | KeyCode::Backspace => Some(BackendSwitchIntent::Clear),
        _ => None,
    }
}

/// History popup navigation (`Enter` closes — handled by the caller).
pub fn history_nav(code: KeyCode) -> Option<HistoryIntent> {
    match code {
        KeyCode::ArrowUp => Some(HistoryIntent::ScrollUp),
        KeyCode::ArrowDown => Some(HistoryIntent::ScrollDown),
        _ => None,
    }
}

/// Settings popup navigation (`Enter` saves + closes — handled by the caller).
pub fn settings_nav(code: KeyCode) -> Option<SettingsIntent> {
    match code {
        KeyCode::ArrowUp => Some(SettingsIntent::MoveUp),
        KeyCode::ArrowDown => Some(SettingsIntent::MoveDown),
        KeyCode::Space => Some(SettingsIntent::Toggle),
        _ => None,
    }
}
