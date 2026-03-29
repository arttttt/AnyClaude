use crate::ui::history::intent::HistoryIntent;
use crate::ui::history::state::HistoryDialogState;
use mvi::{Actor, ActorScope};

pub const MAX_VISIBLE_ROWS: usize = 14;

pub struct HistoryActor;

impl Actor for HistoryActor {
    type State = HistoryDialogState;
    type Intent = HistoryIntent;
    type SideEffect = ();

    fn handle_intent(
        &self,
        intent: Self::Intent,
        scope: &mut ActorScope<Self::State, Self::SideEffect>,
    ) {
        match intent {
            HistoryIntent::Load { entries } => {
                let scroll_offset = entries.len().saturating_sub(MAX_VISIBLE_ROWS);
                scope.reduce(|_| HistoryDialogState::Visible {
                    entries,
                    scroll_offset,
                });
            }
            HistoryIntent::Close => {
                scope.reduce(|_| HistoryDialogState::Hidden);
            }
            HistoryIntent::ScrollUp => {
                scope.reduce(|state| match state {
                    HistoryDialogState::Visible {
                        entries,
                        scroll_offset,
                    } => HistoryDialogState::Visible {
                        entries,
                        scroll_offset: scroll_offset.saturating_sub(1),
                    },
                    other => other,
                });
            }
            HistoryIntent::ScrollDown => {
                scope.reduce(|state| match state {
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
                });
            }
        }
    }
}
