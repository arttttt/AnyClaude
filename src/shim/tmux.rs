//! tmux PATH shim.
//!
//! Intercepts all tmux calls from Claude Code. For `send-keys` commands
//! that spawn a teammate process, injects `ANTHROPIC_BASE_URL` pointing
//! to `/teammate/{agent_id}` on our proxy so the routing layer can
//! identify the teammate and direct traffic to the correct backend.
//!
//! The agent_id is embedded in the URL path (not a header) because
//! URL is the most reliable transport — headers can be stripped by
//! proxies, CDNs, or CC itself.
//!
//! Detection relies on `--agent-id` flag (part of agent teams protocol),
//! not on the binary path — works across all Claude Code installation
//! methods (Homebrew, install.sh, npm, etc.).
//!
//! All other tmux commands are forwarded unchanged to the real binary.

use std::path::Path;

use anyhow::Result;

use super::write_executable;

/// Log file name inside the shim directory.
pub const LOG_FILENAME: &str = "tmux_shim.log";

const TEMPLATE: &str = r#"#!/bin/bash
# AnyClaude tmux shim — intercepts send-keys to inject teammate routing.
#
# Detects teammate spawns by --agent-id flag (agent teams protocol), then
# registers the teammate via /api/teammate-start and replaces
# ANTHROPIC_BASE_URL to route through /teammate/{agent_id} proxy path.
# Agent ID is embedded in the URL (most reliable transport).

SHIM_DIR="$(cd "$(dirname "$0")" && pwd)"
LOG_ENABLED=__LOG_ENABLED__
LOG="$SHIM_DIR/tmux_shim.log"
# Persistent log survives TempDir cleanup
PLOG="$HOME/.config/anyclaude/logs/tmux_shim.log"
mkdir -p "$(dirname "$PLOG")" 2>/dev/null

slog() {
  $LOG_ENABLED || return
  echo "[$(date '+%H:%M:%S.%N')] $1" | tee -a "$LOG" >> "$PLOG"
}

# Find real tmux, skipping our shim directory.
find_real_tmux() {
  local IFS=':'
  for d in $PATH; do
    [ "$d" = "$SHIM_DIR" ] && continue
    [ -x "$d/tmux" ] && echo "$d/tmux" && return
  done
}

# Extract agent_id value from a string containing "--agent-id <value>".
extract_agent_id() {
  printf '%s' "$1" | grep -oE '\-\-agent-id [^ ]+' | head -1 | cut -d' ' -f2
}

REAL_TMUX="$(find_real_tmux)"
if [ -z "$REAL_TMUX" ]; then
  slog "ERROR: real tmux not found"
  echo "tmux: command not found (anyclaude shim)" >&2
  exit 127
fi

# Teammate env vars to inject.
# Uses sed with | delimiter to avoid conflicts with / and : in URLs.
args=()
has_send_keys=false
injected=false
for arg in "$@"; do
  if [ "$arg" = "send-keys" ]; then
    has_send_keys=true
    args+=("$arg")
    continue
  fi

  if $has_send_keys && ! $injected; then
    # Detect teammate spawn by --agent-id flag (part of agent teams protocol,
    # stable across Claude Code versions and installation methods).
    if [[ "$arg" == *"--agent-id "* ]]; then
      slog "BEFORE inject: $(printf '%q' "$arg")"

      # Extract agent_id for URL embedding.
      agent_id=$(extract_agent_id "$arg")
      slog "Extracted agent_id: $agent_id"

      # Register teammate in proxy registry (fire-and-forget, 5s timeout).
      if [ -n "$agent_id" ]; then
        curl -s -m 5 -X POST "http://127.0.0.1:__PORT__/api/teammate-start" \
          -H 'Content-Type: application/json' \
          -d "{\"agent_id\":\"$agent_id\"}" >/dev/null 2>&1
        slog "Registered teammate '$agent_id' via /api/teammate-start"
      fi

      # Agent ID embedded in URL path — most reliable transport.
      INJECT_URL="ANTHROPIC_BASE_URL=http://127.0.0.1:__PORT__/teammate/${agent_id}"
      # Session token header for auth.
      INJECT_HEADERS="ANTHROPIC_CUSTOM_HEADERS=x-session-token:__SESSION_TOKEN__"

      # Strip existing ANTHROPIC_CUSTOM_HEADERS if present (shim re-entry)
      if [[ "$arg" == *ANTHROPIC_CUSTOM_HEADERS=* ]]; then
        arg=$(printf '%s' "$arg" | sed "s|ANTHROPIC_CUSTOM_HEADERS=[^ ]*||")
      fi

      # Replace ANTHROPIC_BASE_URL with teammate URL + inject headers.
      # Anchored on the variable name, not on command structure.
      if [[ "$arg" == *ANTHROPIC_BASE_URL=* ]]; then
        arg=$(printf '%s' "$arg" | sed "s|ANTHROPIC_BASE_URL=[^ ]*|$INJECT_URL $INJECT_HEADERS|")
      else
        # Fallback: no URL in command — inject before --agent-id
        arg=$(printf '%s' "$arg" | sed "s|--agent-id|$INJECT_URL $INJECT_HEADERS --agent-id|")
      fi

      slog "AFTER  inject: $(printf '%q' "$arg")"
      args+=("$arg")
      injected=true
      slog "INJECT teammate route (agent-id detected)"
      continue
    fi
  fi

  args+=("$arg")
done

if $injected; then
  slog "EXEC: $(printf '%q ' "${args[@]}")"
  exec "$REAL_TMUX" "${args[@]}"
else
  slog "tmux $*"
  exec "$REAL_TMUX" "$@"
fi
"#;

/// Install the tmux shim script into `dir`.
pub fn install(dir: &Path, proxy_port: u16, session_token: &str, log_enabled: bool) -> Result<()> {
    let script = TEMPLATE
        .replace("__PORT__", &proxy_port.to_string())
        .replace("__SESSION_TOKEN__", session_token)
        .replace("__LOG_ENABLED__", if log_enabled { "true" } else { "false" });
    write_executable(dir, "tmux", &script)
}
