mod intent;
mod state;

pub use intent::BackendSwitchIntent;
pub use state::{BackendPopupSection, BackendSwitchState};

use crate::config::Backend;

/// Map an override-section selection index into the backend id it represents.
/// Index 0 is the "Disabled" leader (returns `None`); indices `1..=N` map to
/// `backends[i - 1]`. Out-of-range indices fall back to `None` so a stale state
/// never panics. Used by the coordinator when applying a Subagent / Teammate
/// override on Enter.
pub fn override_selection_to_backend_id(backends: &[Backend], selection: usize) -> Option<String> {
    if selection == 0 {
        return None;
    }
    backends.get(selection - 1).map(|b| b.name.clone())
}
