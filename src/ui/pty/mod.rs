//! PTY lifecycle feature module.
//!
//! Manages PTY startup state machine: buffering input until Claude Code
//! is ready to receive it.
//!
//! # Architecture
//!
//! Uses MVI (Model-View-Intent) pattern:
//! - `state.rs` - Lifecycle state enum (Pending → Attached → Ready)
//! - `intent.rs` - System events (Attach, GotOutput, BufferInput)
//! - `reducer.rs` - State transitions (pure, no side effects)

mod intent;
mod reducer;
mod state;

pub use intent::PtyIntent;
pub use reducer::PtyReducer;
pub use state::PtyLifecycleState;
