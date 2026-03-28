//! Intents for the PTY lifecycle.

use mvi::Intent;

/// Intents that can be dispatched to the PTY lifecycle reducer.
#[derive(Debug)]
pub enum PtyIntent {
    /// PTY process has been spawned and attached.
    Attach,

    /// PTY produced its first output — Claude Code is ready.
    GotOutput,

    /// Buffer input while PTY is not yet ready.
    /// In Ready state this is a no-op (caller sends directly to PtyHandle).
    BufferInput {
        bytes: Vec<u8>,
    },

    /// Detach PTY for restart (any → Restarting).
    Detach,

    /// New PTY spawn failed after detach (Restarting → Pending).
    SpawnFailed,
}

impl Intent for PtyIntent {}
