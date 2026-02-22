//! Pipeline — ties all argument processing stages together.

use crate::args::assembler::ArgAssembler;
use crate::args::classifier::classify;
use crate::args::env_builder::EnvSet;
use crate::args::registry::flag_registry;
use crate::args::session::{resolve_session, SessionSource};
use crate::args::SessionMode;
use crate::config::ClaudeSettingsManager;
use crate::shim::TeammateShim;

/// Ready-to-use parameters for spawning the PTY process.
#[derive(Debug, Clone)]
pub struct SpawnParams {
    /// Command to execute (always "claude").
    pub command: String,
    /// CLI arguments for the command.
    pub args: Vec<String>,
    /// Environment variables to set.
    pub env: Vec<(String, String)>,
    /// Resolved session ID.
    pub session_id: String,
    /// Warnings produced during argument processing.
    pub warnings: Vec<String>,
}

/// Build spawn parameters from raw user arguments.
///
/// This is the main entry point for the argument pipeline.
///
/// # Arguments
///
/// * `raw_args` - Raw arguments from the user (after wrapper flags like `--backend` are stripped)
/// * `proxy_url` - The proxy URL to inject as ANTHROPIC_BASE_URL
/// * `settings` - Settings manager for CLI flags and env vars
/// * `shim` - Optional teammate shim for PATH override and --teammate-mode
///
/// # Returns
///
/// A `SpawnParams` struct ready to pass to `PtySession::spawn()`.
pub fn build_spawn_params(
    raw_args: &[String],
    proxy_url: &str,
    settings: &ClaudeSettingsManager,
    shim: Option<&TeammateShim>,
) -> SpawnParams {
    let registry = flag_registry();

    // Stage 1: Classify arguments
    let classified = classify(raw_args, &registry);

    // Stage 2: Resolve session
    let session = resolve_session(&classified.args);

    // Determine SessionMode based on SessionSource
    let session_mode = match session.source {
        SessionSource::ContinueLast | SessionSource::ResumeId => SessionMode::Resume,
        SessionSource::ExplicitId | SessionSource::Generated => SessionMode::Initial,
    };

    // Stage 3: Build environment
    let env = EnvSet::new()
        .with_proxy_url(proxy_url)
        .with_settings(settings)
        .with_shim(shim)
        .build();

    // Stage 4: Assemble arguments
    let args = ArgAssembler::from_passthrough(&classified.args)
        .with_session(&session, session_mode)
        .with_settings(settings)
        .with_teammate_mode(shim)
        .build();

    // Collect all warnings
    let mut warnings = classified.warnings;
    warnings.extend(session.warnings);

    SpawnParams {
        command: "claude".into(),
        args,
        env,
        session_id: session.session_id,
        warnings,
    }
}

/// Build spawn parameters for a restart (PTY restart with new settings).
///
/// Similar to `build_spawn_params`, but accepts pre-computed env vars and CLI args
/// from the settings UI, merging them with the base configuration.
pub fn build_restart_params(
    raw_args: &[String],
    proxy_url: &str,
    settings: &ClaudeSettingsManager,
    shim: Option<&TeammateShim>,
    extra_env: Vec<(String, String)>,
    extra_args: Vec<String>,
) -> SpawnParams {
    let registry = flag_registry();

    // Stage 1: Classify arguments
    let classified = classify(raw_args, &registry);

    // Stage 2: Resolve session
    let session = resolve_session(&classified.args);

    // Determine SessionMode based on SessionSource
    let session_mode = match session.source {
        SessionSource::ContinueLast | SessionSource::ResumeId => SessionMode::Resume,
        SessionSource::ExplicitId | SessionSource::Generated => SessionMode::Initial,
    };

    // Stage 3: Build environment (with extra)
    let env = EnvSet::new()
        .with_proxy_url(proxy_url)
        .with_settings(settings)
        .with_shim(shim)
        .with_extra(extra_env)
        .build();

    // Stage 4: Assemble arguments (with extra)
    let args = ArgAssembler::from_passthrough(&classified.args)
        .with_session(&session, session_mode)
        .with_settings(settings)
        .with_teammate_mode(shim)
        .with_extra(extra_args)
        .build();

    // Collect all warnings
    let mut warnings = classified.warnings;
    warnings.extend(session.warnings);

    SpawnParams {
        command: "claude".into(),
        args,
        env,
        session_id: session.session_id,
        warnings,
    }
}
