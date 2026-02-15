//! Environment builder — all env vars in one place.

use crate::config::ClaudeSettingsManager;
use crate::shim::TeammateShim;

/// Builder for environment variables passed to the spawned process.
#[derive(Debug, Clone)]
pub struct EnvSet {
    vars: Vec<(String, String)>,
}

impl EnvSet {
    /// Create an empty environment set.
    pub fn new() -> Self {
        Self { vars: Vec::new() }
    }

    /// Always-present: proxy URL for Claude API.
    pub fn with_proxy_url(mut self, url: &str) -> Self {
        self.vars
            .push(("ANTHROPIC_BASE_URL".into(), url.into()));
        self
    }

    /// From settings manager (agent teams, etc.)
    pub fn with_settings(mut self, settings: &ClaudeSettingsManager) -> Self {
        self.vars.extend(settings.to_env_vars());
        self
    }

    /// From teammate shim (PATH override).
    pub fn with_shim(mut self, shim: Option<&TeammateShim>) -> Self {
        if let Some(s) = shim {
            self.vars.push(s.path_env());
        }
        self
    }

    /// Add arbitrary extra environment variables.
    pub fn with_extra(mut self, extra: Vec<(String, String)>) -> Self {
        self.vars.extend(extra);
        self
    }

    /// Build the final environment variable list.
    pub fn build(self) -> Vec<(String, String)> {
        self.vars
    }
}

impl Default for EnvSet {
    fn default() -> Self {
        Self::new()
    }
}
