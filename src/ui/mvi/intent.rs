//! Base trait for intents (user/system actions) in MVI architecture.

/// Marker trait for intent objects.
///
/// Intents represent:
/// - User actions (button clicks, key presses)
/// - System events (API responses, timers)
/// - Navigation events
///
/// Intents are processed by reducers to produce new states.
pub trait Intent: Send + 'static {}
