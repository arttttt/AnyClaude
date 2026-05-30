use crate::config::SettingsFieldSnapshot;

/// Settings popup intents — the message vocabulary consumed by
/// [`SettingsDialogState::apply`]. Plain enum (no MVI traits).
#[derive(Debug, Clone)]
pub enum SettingsIntent {
    Load { fields: Vec<SettingsFieldSnapshot> },
    Close,
    /// User pressed Escape. If dirty and not yet confirming, sets confirm_discard flag.
    /// If clean or already confirming, transitions to Hidden.
    RequestClose,
    MoveUp,
    MoveDown,
    Toggle,
}
