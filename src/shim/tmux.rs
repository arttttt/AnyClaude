//! tmux PATH shim.
//!
//! Intercepts all tmux calls from Claude Code. Logs every invocation
//! to `tmux_shim.log` in the shim directory, then delegates to the
//! real tmux binary.
//!
//! Future: handle teammate spawning directly without real tmux.

use std::path::Path;

use anyhow::Result;

use super::write_executable;

/// Log file name inside the shim directory.
pub const LOG_FILENAME: &str = "tmux_shim.log";

const TEMPLATE: &str = r#"#!/bin/bash
# AnyClaude tmux shim.
# Logs all tmux invocations from Claude Code for analysis,
# then delegates to the real tmux binary.

SHIM_DIR="$(cd "$(dirname "$0")" && pwd)"
LOG="$SHIM_DIR/tmux_shim.log"
echo "[$(date '+%H:%M:%S.%N')] tmux $*" >> "$LOG"

# Find real tmux, skipping our shim directory.
find_real_tmux() {
  local IFS=':'
  for d in $PATH; do
    [ "$d" = "$SHIM_DIR" ] && continue
    [ -x "$d/tmux" ] && echo "$d/tmux" && return
  done
}

REAL_TMUX="$(find_real_tmux)"
if [ -z "$REAL_TMUX" ]; then
  echo "[$(date '+%H:%M:%S.%N')] ERROR: real tmux not found" >> "$LOG"
  echo "tmux: command not found (anyclaude shim)" >&2
  exit 127
fi

echo "[$(date '+%H:%M:%S.%N')] -> $REAL_TMUX $*" >> "$LOG"
exec "$REAL_TMUX" "$@"
"#;

/// Install the tmux shim script into `dir`.
pub fn install(dir: &Path) -> Result<()> {
    write_executable(dir, "tmux", TEMPLATE)
}
