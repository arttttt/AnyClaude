use crate::ui::history::intent::HistoryIntent;
use crate::ui::history::state::HistoryDialogState;
use crate::ui::mvi::Reducer;

pub const MAX_VISIBLE_ROWS: usize = 14;

pub struct HistoryReducer;

impl Reducer for HistoryReducer {
    type State = HistoryDialogState;
    type Intent = HistoryIntent;

    fn reduce(state: Self::State, intent: Self::Intent) -> Self::State {
        match intent {
            HistoryIntent::Load { entries } => {
                let scroll_offset = entries.len().saturating_sub(MAX_VISIBLE_ROWS);
                HistoryDialogState::Visible {
                    entries,
                    scroll_offset,
                }
            }
            HistoryIntent::Close => HistoryDialogState::Hidden,
            HistoryIntent::ScrollUp => match state {
                HistoryDialogState::Visible {
                    entries,
                    scroll_offset,
                } => HistoryDialogState::Visible {
                    entries,
                    scroll_offset: scroll_offset.saturating_sub(1),
                },
                other => other,
            },
            HistoryIntent::ScrollDown => match state {
                HistoryDialogState::Visible {
                    entries,
                    scroll_offset,
                } => {
                    let max_offset = entries.len().saturating_sub(MAX_VISIBLE_ROWS);
                    HistoryDialogState::Visible {
                        scroll_offset: (scroll_offset + 1).min(max_offset),
                        entries,
                    }
                }
                other => other,
            },
        }
    }
}
