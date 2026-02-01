//! Thinking block transformation system.
//!
//! This module provides a flexible system for handling Claude's thinking blocks
//! when proxying requests to different backends.
//!
//! # Modes
//!
//! - **Strip**: Remove thinking blocks entirely (simplest, most compatible)
//! - **Summarize**: Keep native during work, summarize on backend switch
//! - **Native**: Keep native format, requires handoff on switch (future)
//!
//! # Architecture
//!
//! ```text
//! ThinkingTransformer (trait)
//!         ▲
//!         │
//!    ┌────┴────┬────────────┐
//!    │         │            │
//! Strip   Summarize    Native
//!    │         │            │
//!    └────┬────┴────────────┘
//!         │
//!         ▼
//! TransformerRegistry (creates & stores active transformer)
//! ```

mod context;
mod error;
mod strip;
mod traits;

pub use context::{TransformContext, TransformResult, TransformStats};
pub use error::TransformError;
pub use strip::StripTransformer;
pub use traits::ThinkingTransformer;

use crate::config::ThinkingMode;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Registry that creates and manages the active thinking transformer.
///
/// Supports hot-swapping transformers when configuration changes.
pub struct TransformerRegistry {
    current: RwLock<Arc<dyn ThinkingTransformer>>,
    mode: std::sync::RwLock<ThinkingMode>,
}

impl TransformerRegistry {
    /// Create a new registry with the given thinking mode.
    pub fn new(mode: ThinkingMode) -> Self {
        let transformer = Self::create_transformer(&mode);
        Self {
            current: RwLock::new(transformer),
            mode: std::sync::RwLock::new(mode),
        }
    }

    /// Create a transformer for the given mode.
    fn create_transformer(mode: &ThinkingMode) -> Arc<dyn ThinkingTransformer> {
        match mode {
            ThinkingMode::Strip => Arc::new(StripTransformer),

            // Future modes - fall back to Strip for now
            ThinkingMode::Summarize => {
                tracing::warn!(
                    "ThinkingMode::Summarize is not yet implemented, using Strip instead"
                );
                Arc::new(StripTransformer)
            }
            ThinkingMode::Native => {
                tracing::warn!(
                    "ThinkingMode::Native is not yet implemented, using Strip instead"
                );
                Arc::new(StripTransformer)
            }

            // Legacy modes - deprecated, fall back to strip with warning
            ThinkingMode::DropSignature => {
                tracing::warn!(
                    "ThinkingMode::DropSignature is deprecated, using Strip instead"
                );
                Arc::new(StripTransformer)
            }
            ThinkingMode::ConvertToText => {
                tracing::warn!(
                    "ThinkingMode::ConvertToText is deprecated, using Strip instead"
                );
                Arc::new(StripTransformer)
            }
            ThinkingMode::ConvertToTags => {
                tracing::warn!(
                    "ThinkingMode::ConvertToTags is deprecated, using Strip instead. \
                     This mode caused context accumulation issues."
                );
                Arc::new(StripTransformer)
            }
        }
    }

    /// Get the current transformer.
    ///
    /// Returns a clone of the Arc'd transformer, so the caller owns a reference
    /// and no lock is held after this returns.
    pub async fn get(&self) -> Arc<dyn ThinkingTransformer> {
        self.current.read().await.clone()
    }

    /// Update the thinking mode (hot-swap transformer).
    pub async fn update_mode(&self, mode: ThinkingMode) {
        // Check and update mode with sync lock, then drop it before async work
        let needs_transformer_update = {
            let mut current_mode = self.mode.write().expect("mode lock poisoned");
            if *current_mode != mode {
                tracing::info!(
                    old_mode = ?*current_mode,
                    new_mode = ?mode,
                    "Switching thinking transformer"
                );
                *current_mode = mode.clone();
                true
            } else {
                false
            }
        }; // sync lock dropped here

        // Now do async work without holding the sync lock
        if needs_transformer_update {
            let transformer = Self::create_transformer(&mode);
            *self.current.write().await = transformer;
        }
    }

    /// Get the current mode.
    pub fn current_mode(&self) -> ThinkingMode {
        self.mode.read().expect("mode lock poisoned").clone()
    }
}

impl std::fmt::Debug for TransformerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformerRegistry")
            .field("mode", &*self.mode.read().expect("mode lock poisoned"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_creates_strip_transformer() {
        let registry = TransformerRegistry::new(ThinkingMode::Strip);
        let transformer = registry.get().await;
        assert_eq!(transformer.name(), "strip");
    }

    #[tokio::test]
    async fn registry_hot_swaps_transformer() {
        let registry = TransformerRegistry::new(ThinkingMode::Strip);
        assert_eq!(registry.current_mode(), ThinkingMode::Strip);

        // Hot swap (currently both map to Strip, but the mechanism works)
        registry.update_mode(ThinkingMode::DropSignature).await;
        // Mode changed but transformer is still Strip (deprecated fallback)
        let transformer = registry.get().await;
        assert_eq!(transformer.name(), "strip");
    }
}
