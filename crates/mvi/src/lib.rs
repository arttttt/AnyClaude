//! Model-View-Intent (MVI) architecture primitives.
//!
//! This module provides base traits for implementing unidirectional
//! data flow in the UI layer.
//!
//! # Architecture
//!
//! ```text
//! Intent ──→ Reducer ──→ State ──→ View
//!    ↑                              │
//!    └──────────────────────────────┘
//! ```
//!
//! - **State**: Immutable representation of UI state
//! - **Intent**: User actions or system events
//! - **Reducer**: Pure function that transforms state based on intents

/// Marker trait for UI state objects.
///
/// States should be:
/// - Immutable (Clone to create new states)
/// - Self-contained (all data needed to render the view)
/// - Comparable (PartialEq for detecting changes)
pub trait UiState: Clone + PartialEq + Default + Send + 'static {}

/// Marker trait for intent objects.
///
/// Intents represent:
/// - User actions (button clicks, key presses)
/// - System events (API responses, timers)
/// - Navigation events
///
/// Intents are processed by reducers to produce new states.
pub trait Intent: Send + 'static {}

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
