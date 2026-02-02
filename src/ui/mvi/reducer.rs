//! Reducer trait for MVI architecture.

use super::intent::Intent;
use super::state::UiState;

/// Reducer transforms state based on intents.
///
/// The reducer is the only place where state transitions happen.
/// It must be a pure function: (State, Intent) -> State
pub trait Reducer {
    /// The state type this reducer operates on.
    type State: UiState;

    /// The intent type this reducer handles.
    type Intent: Intent;

    /// Process an intent and return the new state.
    ///
    /// This should be a pure function with no side effects.
    fn reduce(state: Self::State, intent: Self::Intent) -> Self::State;
}
