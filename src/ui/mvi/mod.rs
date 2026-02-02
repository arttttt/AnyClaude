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

mod intent;
mod reducer;
mod state;

pub use intent::Intent;
pub use reducer::Reducer;
pub use state::UiState;
