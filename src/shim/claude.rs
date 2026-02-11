//! Claude binary PATH shim.
//!
//! When `CLAUDE_CODE_AGENT_TYPE` is set (teammate subprocess), rewrites
//! `ANTHROPIC_BASE_URL` to add a `/teammate` prefix so the proxy routing
//! layer can distinguish teammate traffic from lead traffic.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::write_executable;

// Assumption: CLAUDE_CODE_AGENT_TYPE is set ONLY for teammate subprocesses,
// not for the lead process. The lead goes through this shim too (same PATH),
// but the env var is absent so the if-block is skipped.
const TEMPLATE: &str = r#"#!/bin/bash
# AnyClaude routing shim.
# Intercepts Claude Code subprocess spawns to modify
# ANTHROPIC_BASE_URL based on environment variables.

if [ -n "$CLAUDE_CODE_AGENT_TYPE" ]; then
  export ANTHROPIC_BASE_URL="http://127.0.0.1:__PORT__/teammate"
fi

exec "__REAL_CLAUDE__" "$@"
"#;

/// Install the claude shim script into `dir`.
pub fn install(dir: &Path, proxy_port: u16) -> Result<()> {
    let real_claude = resolve_real_claude()
        .context("cannot create teammate shim: claude binary not found in PATH")?;

    let script = TEMPLATE
        .replace("__PORT__", &proxy_port.to_string())
        .replace("__REAL_CLAUDE__", &real_claude.to_string_lossy());

    write_executable(dir, "claude", &script)
}

/// Find the real `claude` binary in PATH.
fn resolve_real_claude() -> Result<PathBuf> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join("claude");
        if is_executable(&candidate) {
            return Ok(candidate);
        }
    }
    anyhow::bail!("'claude' not found in PATH")
}

/// Check if a path exists and is executable.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.is_file()
            && std::fs::metadata(path)
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}
