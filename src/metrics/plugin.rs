use super::types::{BackendOverride, PostResponseContext, PreRequestContext};

pub trait ObservabilityPlugin: Send + Sync {
    fn pre_request(&self, _ctx: &mut PreRequestContext<'_>) -> Option<BackendOverride> {
        None
    }

    fn post_response(&self, _ctx: &mut PostResponseContext<'_>) {}
}
