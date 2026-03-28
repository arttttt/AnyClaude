//! State for the PTY lifecycle.

use mvi::UiState;
use std::collections::VecDeque;

/// PTY lifecycle state machine.
///
/// Tracks the startup sequence: PTY not yet spawned → PTY attached but
/// Claude Code not ready → Claude Code ready (produced output).
/// Input is buffered until Ready to prevent message loss during startup.
#[derive(Debug, Clone, PartialEq)]
pub enum PtyLifecycleState {
    /// PTY not yet attached, buffering input.
    Pending {
        buffer: VecDeque<Vec<u8>>,
    },

    /// PTY attached but Claude Code not ready (no output yet).
    Attached {
        buffer: VecDeque<Vec<u8>>,
    },

    /// Claude Code ready (produced output), input goes directly to PTY.
    Ready,

    /// PTY is being restarted (old PTY shut down, new one not yet attached).
    /// Input is dropped. ProcessExit from old PTY is ignored.
    Restarting,
}

impl Default for PtyLifecycleState {
    fn default() -> Self {
        PtyLifecycleState::Pending {
            buffer: VecDeque::new(),
        }
    }
}

impl UiState for PtyLifecycleState {}

impl PtyLifecycleState {
    /// Check if PTY is ready to receive input directly.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Check if input is being buffered (Pending or Attached).
    pub fn is_buffering(&self) -> bool {
        matches!(self, Self::Pending { .. } | Self::Attached { .. })
    }

    /// Check if PTY is attached (but not necessarily ready yet).
    pub fn is_attached(&self) -> bool {
        matches!(self, Self::Attached { .. })
    }

    /// Check if PTY is in the middle of a restart cycle.
    pub fn is_restarting(&self) -> bool {
        matches!(self, Self::Restarting)
    }
}
