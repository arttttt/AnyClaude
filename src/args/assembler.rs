//! Argument assembler — all CLI args in one place.

use crate::args::classifier::ClassifiedArg;
use crate::args::session::SessionResolution;
use crate::args::SessionMode;
use crate::config::ClaudeSettingsManager;
use crate::shim::TeammateShim;

/// Builder for CLI arguments passed to the spawned claude process.
#[derive(Debug, Clone)]
pub struct ArgAssembler {
    args: Vec<String>,
}

impl ArgAssembler {
    /// Start with passthrough args (filtered from classified args).
    ///
    /// Wrapper-owned and intercepted flags are excluded — they've been consumed.
    pub fn from_passthrough(classified: &[ClassifiedArg]) -> Self {
        let args = classified
            .iter()
            .filter_map(|a| match a {
                ClassifiedArg::KnownPassthrough { flag, value } => {
                    let mut v = vec![flag.clone()];
                    if let Some(val) = value {
                        v.push(val.clone());
                    }
                    Some(v)
                }
                ClassifiedArg::UnknownPassthrough(s) => Some(vec![s.clone()]),
                ClassifiedArg::Positional(s) => Some(vec![s.clone()]),
                // Wrapper-owned and Intercepted are consumed
                ClassifiedArg::WrapperOwned { .. } | ClassifiedArg::Intercepted { .. } => None,
            })
            .flatten()
            .collect();
        Self { args }
    }

    /// Start with an empty arg list.
    pub fn new() -> Self {
        Self { args: Vec::new() }
    }

    /// Inject session flag based on mode.
    pub fn with_session(mut self, session: &SessionResolution, mode: SessionMode) -> Self {
        match mode {
            SessionMode::Initial => {
                self.args.push("--session-id".into());
                self.args.push(session.session_id.clone());
            }
            SessionMode::Resume => {
                self.args.push("--resume".into());
                self.args.push(session.session_id.clone());
            }
        }
        self
    }

    /// From settings manager (CLI flags from registry).
    pub fn with_settings(mut self, settings: &ClaudeSettingsManager) -> Self {
        self.args.extend(settings.to_cli_args());
        self
    }

    /// From teammate shim (--teammate-mode tmux).
    pub fn with_teammate_mode(mut self, shim: Option<&TeammateShim>) -> Self {
        if shim.is_some() {
            self.args.push("--teammate-mode".into());
            self.args.push("tmux".into());
        }
        self
    }

    /// Add arbitrary extra arguments.
    pub fn with_extra(mut self, extra: Vec<String>) -> Self {
        self.args.extend(extra);
        self
    }

    /// Build the final argument list.
    pub fn build(self) -> Vec<String> {
        self.args
    }
}

impl Default for ArgAssembler {
    fn default() -> Self {
        Self::new()
    }
}
