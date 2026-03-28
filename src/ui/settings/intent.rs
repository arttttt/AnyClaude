use crate::config::SettingsFieldSnapshot;
use mvi::Intent;

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

impl Intent for SettingsIntent {}
