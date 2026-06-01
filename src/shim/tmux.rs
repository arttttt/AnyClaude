//! tmux PATH shim.
//!
//! Intercepts every tmux call from Claude Code and forwards it to the proxy's
//! `/api/tmux` control plane — anyclaude IS the tmux, there is no real tmux
//! server. The `TmuxAdapter` behind that endpoint turns each verb into a
//! teammate-pane lifecycle event (`split-window` → a pane, `kill-pane` →
//! removal, `send-keys` → typing into the pane's PTY, …) and replies the pane
//! `%N` for synchronous verbs, which the shim prints to stdout like real tmux.
//!
//! `send-keys` additionally gets the teammate routing rewrite: the spawned
//! teammate's `ANTHROPIC_BASE_URL` is repointed at `/teammate/{agent_id}` on our
//! proxy (and `/api/teammate-start` records the agent → backend mapping) so its
//! API traffic is identified and routed to the right backend. The agent_id is
//! embedded in the URL path (not a header) because the URL is the most reliable
//! transport — headers can be stripped by proxies, CDNs, or CC itself.
//!
//! Detection of a teammate spawn relies on the `--agent-id` flag (agent teams
//! protocol), not the binary path — so it works across all Claude Code
//! installation methods (Homebrew, install.sh, npm, …).

use std::path::Path;

use anyhow::Result;

use super::write_executable;

/// Log file name inside the shim directory.
pub const LOG_FILENAME: &str = "tmux_shim.log";

const TEMPLATE: &str = r#"#!/bin/bash
# AnyClaude tmux shim — forwards every tmux verb to the proxy's /api/tmux control
# plane (there is no real tmux; anyclaude renders the panes itself). send-keys
# additionally gets the teammate ANTHROPIC_BASE_URL / header rewrite so the
# teammate's API traffic routes through our proxy by agent_id.

SHIM_DIR="$(cd "$(dirname "$0")" && pwd)"
LOG_ENABLED=__LOG_ENABLED__
LOG="$SHIM_DIR/tmux_shim.log"
# Persistent log survives TempDir cleanup.
PLOG="$HOME/.config/anyclaude/logs/tmux_shim.__SESSION_ID__.log"
mkdir -p "$(dirname "$PLOG")" 2>/dev/null

slog() {
  $LOG_ENABLED || return
  echo "[$(date '+%H:%M:%S.%N')] $1" | tee -a "$LOG" >> "$PLOG"
}

# Extract agent_id value from a string containing "--agent-id <value>".
extract_agent_id() {
  printf '%s' "$1" | grep -oE '\-\-agent-id [^ ]+' | head -1 | cut -d' ' -f2
}

# JSON-escape a string for embedding in a JSON string literal.
json_escape() {
  local s=$1
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\t'/\\t}
  s=${s//$'\r'/\\r}
  s=${s//$'\n'/\\n}
  printf '%s' "$s"
}

# Rewrite a send-keys teammate spawn: register the agent and inject the
# teammate-routed ANTHROPIC_BASE_URL + session-token header into the keystrokes.
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

  if $has_send_keys && ! $injected && [[ "$arg" == *"--agent-id "* ]]; then
    slog "BEFORE inject: $(printf '%q' "$arg")"

    agent_id=$(extract_agent_id "$arg")
    slog "Extracted agent_id: $agent_id"

    # Register teammate in the proxy registry (fire-and-forget, 5s timeout).
    if [ -n "$agent_id" ]; then
      curl -s -m 5 -X POST "http://127.0.0.1:__PORT__/api/teammate-start" \
        -H 'Content-Type: application/json' \
        -d "{\"agent_id\":\"$agent_id\"}" >/dev/null 2>&1
      slog "Registered teammate '$agent_id' via /api/teammate-start"
    fi

    # Agent ID embedded in the URL path — most reliable transport.
    INJECT_URL="ANTHROPIC_BASE_URL=http://127.0.0.1:__PORT__/teammate/${agent_id}"
    INJECT_HEADERS="ANTHROPIC_CUSTOM_HEADERS=x-session-token:__SESSION_TOKEN__"

    # Strip any existing ANTHROPIC_CUSTOM_HEADERS (shim re-entry).
    if [[ "$arg" == *ANTHROPIC_CUSTOM_HEADERS=* ]]; then
      arg=$(printf '%s' "$arg" | sed "s|ANTHROPIC_CUSTOM_HEADERS=[^ ]*||")
    fi

    # Replace ANTHROPIC_BASE_URL with the teammate URL + inject headers,
    # anchored on the variable name, not on command structure.
    if [[ "$arg" == *ANTHROPIC_BASE_URL=* ]]; then
      arg=$(printf '%s' "$arg" | sed "s|ANTHROPIC_BASE_URL=[^ ]*|$INJECT_URL $INJECT_HEADERS|")
    else
      arg=$(printf '%s' "$arg" | sed "s|--agent-id|$INJECT_URL $INJECT_HEADERS --agent-id|")
    fi

    slog "AFTER  inject: $(printf '%q' "$arg")"
    args+=("$arg")
    injected=true
    continue
  fi

  args+=("$arg")
done

# Build {"args":[...]} from the (rewritten) argv and POST to the control plane.
body='{"args":['
first=true
for a in "${args[@]}"; do
  $first || body+=','
  first=false
  body+="\"$(json_escape "$a")\""
done
body+=']}'

slog "POST /api/tmux: ${args[*]}"

resp=$(curl -s -m 10 -w $'\n%{http_code}' \
  -X POST "http://127.0.0.1:__PORT__/api/tmux" \
  -H 'Content-Type: application/json' \
  -d "$body" 2>/dev/null)
code=${resp##*$'\n'}
out=${resp%$'\n'*}

slog "<- HTTP $code: $out"

# Print the response body (split-window -P returns "%N" on stdout, like tmux).
[ -n "$out" ] && printf '%s\n' "$out"

# Map the HTTP status to an exit code (tmux returns 0 on success).
case "$code" in
  2*) exit 0 ;;
  *)
    echo "anyclaude tmux: '$*' failed (HTTP ${code:-000})" >&2
    exit 1
    ;;
esac
"#;

/// Install the tmux shim script into `dir`.
///
/// Writes one file — `tmux`, the executable bash shim that forwards every tmux
/// call to the proxy's `/api/tmux` control plane (no real tmux server, no
/// `tmux.conf`: anyclaude renders the panes itself).
pub fn install(
    dir: &Path,
    proxy_port: u16,
    session_token: &str,
    session_id: &str,
    log_enabled: bool,
) -> Result<()> {
    let script = TEMPLATE
        .replace("__PORT__", &proxy_port.to_string())
        .replace("__SESSION_TOKEN__", session_token)
        .replace("__SESSION_ID__", session_id)
        .replace("__LOG_ENABLED__", if log_enabled { "true" } else { "false" });
    write_executable(dir, "tmux", &script)?;
    Ok(())
}
