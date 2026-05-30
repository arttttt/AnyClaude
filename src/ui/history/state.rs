use std::time::SystemTime;

use crate::ui::history::intent::HistoryIntent;

/// Max history rows visible at once — drives the initial scroll-to-end and the
/// scroll-down clamp.
pub const MAX_VISIBLE_ROWS: usize = 14;

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub timestamp: SystemTime,
    pub from_backend: Option<String>,
    pub to_backend: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum HistoryDialogState {
    #[default]
    Hidden,
    Visible {
        entries: Vec<HistoryEntry>,
        scroll_offset: usize,
    },
}

impl HistoryDialogState {
    pub fn is_visible(&self) -> bool {
        !matches!(self, Self::Hidden)
    }

    /// The single authoritative transition (the plain-fn replacement for the
    /// old MVI `Actor::handle_intent` — same semantics, mutated in place).
    pub fn apply(&mut self, intent: HistoryIntent) {
        match intent {
            HistoryIntent::Load { entries } => {
                // Open scrolled to the most-recent rows.
                let scroll_offset = entries.len().saturating_sub(MAX_VISIBLE_ROWS);
                *self = HistoryDialogState::Visible {
                    entries,
                    scroll_offset,
                };
            }
            HistoryIntent::Close => *self = HistoryDialogState::Hidden,
            HistoryIntent::ScrollUp => {
                if let HistoryDialogState::Visible { scroll_offset, .. } = self {
                    *scroll_offset = scroll_offset.saturating_sub(1);
                }
            }
            HistoryIntent::ScrollDown => {
                if let HistoryDialogState::Visible {
                    entries,
                    scroll_offset,
                } = self
                {
                    let max_offset = entries.len().saturating_sub(MAX_VISIBLE_ROWS);
                    *scroll_offset = (*scroll_offset + 1).min(max_offset);
                }
            }
        }
    }
}
