//! PATH shim for intercepting Claude Code teammate process spawns.
//!
//! A tmux shim is placed in a temp directory prepended to PATH.
//! It intercepts `send-keys` commands that launch teammate claude processes
//! and injects `ANTHROPIC_BASE_URL` pointing to the `/teammate` proxy route.
//!
//! Self-contained: no dependencies on proxy, axum, or routing internals.

mod tmux;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Owns the temp directory containing the shim scripts.
/// The directory (and all shim scripts) is cleaned up on drop.
pub struct TeammateShim {
    _dir: tempfile::TempDir,
    dir_path: PathBuf,
}

impl TeammateShim {
    /// Create shim scripts (claude + tmux) in a temp directory.
    pub fn create(proxy_port: u16) -> Result<Self> {
        let dir = tempfile::tempdir()
            .context("failed to create temp directory for teammate shims")?;

        tmux::install(dir.path(), proxy_port)?;

        let dir_path = dir.path().to_owned();
        Ok(Self { _dir: dir, dir_path })
    }

    /// Returns a `("PATH", "shim_dir:$PATH")` tuple for use with
    /// `PtySpawnConfig::build(extra_env)`.
    pub fn path_env(&self) -> (String, String) {
        let current = std::env::var("PATH").unwrap_or_default();
        (
            "PATH".to_string(),
            format!("{}:{}", self.dir_path.display(), current),
        )
    }

    /// Path to the tmux shim log file (may not exist yet).
    pub fn tmux_log_path(&self) -> PathBuf {
        self.dir_path.join(tmux::LOG_FILENAME)
    }
}

/// Write a script file and make it executable.
fn write_executable(dir: &Path, name: &str, content: &str) -> Result<()> {
    let path = dir.join(name);
    std::fs::write(&path, content)
        .with_context(|| format!("failed to write {name} shim script"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to make {name} shim executable"))?;
    }

    Ok(())
}
