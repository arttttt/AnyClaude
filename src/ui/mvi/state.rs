//! Base trait for UI state in MVI architecture.

/// Marker trait for UI state objects.
///
/// States should be:
/// - Immutable (Clone to create new states)
/// - Self-contained (all data needed to render the view)
/// - Comparable (PartialEq for detecting changes)
pub trait UiState: Clone + PartialEq + Default + Send + 'static {}
