//! State for the PTY lifecycle.

use crate::ui::mvi::UiState;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_pending() {
        let state = PtyLifecycleState::default();
        assert!(matches!(state, PtyLifecycleState::Pending { buffer } if buffer.is_empty()));
    }

    #[test]
    fn is_ready_check() {
        assert!(!PtyLifecycleState::default().is_ready());
        assert!(!PtyLifecycleState::Attached { buffer: VecDeque::new() }.is_ready());
        assert!(PtyLifecycleState::Ready.is_ready());
    }

    #[test]
    fn is_buffering_check() {
        assert!(PtyLifecycleState::default().is_buffering());
        assert!(PtyLifecycleState::Attached { buffer: VecDeque::new() }.is_buffering());
        assert!(!PtyLifecycleState::Ready.is_buffering());
    }
}
