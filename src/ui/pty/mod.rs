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
//! - `actor.rs` - State transitions via Actor

mod actor;
mod intent;
mod state;

pub use actor::{PtyActor, PtySideEffect};
pub use intent::PtyIntent;
pub use state::PtyLifecycleState;
