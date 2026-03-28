# AnyClaude

TUI wrapper for Claude Code with hot-swappable backend support and transparent API proxying.

**Goal:** Make switching between API providers effortless. Configure all your backends once, then switch between them with a single hotkey — no config edits, no restarts, no interruptions.

**Note:** Only Anthropic API-compatible backends are supported.

## Why?

Claude Code is great, but sometimes you need a different provider — maybe Anthropic is down, rate-limited, or you want to use another Anthropic-compatible backend. Without AnyClaude, switching means editing config files or environment variables every time.

AnyClaude solves this:

- Configure all backends once
- Switch with `Ctrl+B` mid-session
- No restarts, no config edits

## Features

- **Hot-Swap Backends** — Switch between providers without restarting Claude
- **Agent Routing** — Route teammates and subagents to separate backends with session affinity
- **Thinking Block Filtering** — Automatic filtering of previous backend's thinking blocks on switch
- **Adaptive Thinking Conversion** — Convert adaptive thinking to enabled format for non-Anthropic backends (`thinking_compat`)
- **Model Mapping** — Remap model names per backend (`model_opus`, `model_sonnet`, `model_haiku`)
- **Transparent Proxy** — Routes API requests through active backend
- **Backend History** — View switch history with `Ctrl+H`
- **Debug Logging** — Request/response logging with configurable detail levels

## Architecture

```
┌─────────────────────────────┐
│        AnyClaude TUI        │
└──────────────┬──────────────┘
               │
        ┌──────▼──────┐
        │ Claude Code │ (main + subagents + teammates)
        └──────┬──────┘
               │ ANTHROPIC_BASE_URL
        ┌──────▼──────┐
        │  7-Stage    │
        │  Pipeline   │
        └─┬─────┬───┬─┘
          │     │   │
    /v1/* │     │   │ /teammate/{id}/v1/*
          │     │   │
   ┌──────▼┐ ┌─▼──────────┐ ┌─▼──────────┐
   │Active │ │  Subagent   │ │  Teammate   │
   │Backend│ │  Backend    │ │  Backend    │
   └───────┘ └─────────────┘ └─────────────┘
```

The main agent's requests go through the active backend (switchable via `Ctrl+B`).
Subagents are pinned to a backend via session affinity (AC markers in CC hooks).
Teammate agents are routed via `/teammate/{agent_id}` URL path prefix.

## Installation

```bash
cargo install --path .
```

Or build manually:

```bash
cargo build --release
# binary at ./target/release/anyclaude
```

## Usage

The wrapper automatically:
1. Starts a local proxy (port auto-assigned starting from configured `bind_addr`)
2. Sets `ANTHROPIC_BASE_URL` environment variable
3. Spawns Claude Code in an embedded terminal
4. Routes all API requests through the active backend

### Hotkeys

| Key | Action |
|-----|--------|
| `Ctrl+B` | Backend switcher popup |
| `Ctrl+H` | Backend switch history |
| `Ctrl+E` | Settings dialog |
| `Ctrl+R` | Restart Claude Code (preserves session) |
| `Ctrl+Q` | Quit |
| `1-9` | Quick-select backend (in switcher) |

## Configuration

Config location: `~/.config/anyclaude/config.toml`

### Minimal Example

```toml
[defaults]
active = "anthropic"

[[backends]]
name = "anthropic"
display_name = "Anthropic"
base_url = "https://api.anthropic.com"
auth_type = "passthrough"  # Forward Claude Code's auth headers

[[backends]]
name = "alternative"
display_name = "Alternative Provider"
base_url = "https://your-provider.com/api"
auth_type = "bearer"
api_key = "your-api-key"
```

### Full Example

```toml
[defaults]
active = "anthropic"              # Default backend at startup
timeout_seconds = 300             # Overall request timeout
connect_timeout_seconds = 5       # TCP connection timeout
idle_timeout_seconds = 60         # Streaming response idle timeout
pool_idle_timeout_seconds = 90    # Connection pool idle timeout
pool_max_idle_per_host = 8        # Max idle connections per host
max_retries = 3                   # Connection retry attempts
retry_backoff_base_ms = 100       # Base backoff for retries (exponential)

[proxy]
bind_addr = "127.0.0.1:8080"      # Local proxy listen address (auto-increments if busy)
base_url = "http://127.0.0.1:8080" # Base URL exposed to Claude Code

[terminal]
scrollback_lines = 10000          # History buffer size

[debug_logging]
level = "verbose"                 # "off", "basic", "verbose", "full"
format = "console"                # "console", "json"
destination = "file"              # "stderr", "file", "both"
file_path = "~/.config/anyclaude/logs/debug.log"
body_preview_bytes = 1024         # Max bytes of request/response body to log
header_preview = true             # Log request/response headers
full_body = false                 # Log full bodies (no size limit)
pretty_print = true               # Pretty-print JSON bodies

[debug_logging.rotation]
mode = "none"                     # "none", "size", "daily"
max_bytes = 10485760              # Max log file size before rotation (10 MB)
max_files = 5                     # Max rotated log files to keep

[[backends]]
name = "anthropic"
display_name = "Anthropic"
base_url = "https://api.anthropic.com"
auth_type = "passthrough"         # Forward Claude Code's auth headers

[[backends]]
name = "alternative"
display_name = "Alternative Provider"
base_url = "https://your-provider.com/api"
auth_type = "bearer"
api_key = "your-api-key"
thinking_compat = true            # Convert adaptive->enabled thinking for this backend
thinking_budget_tokens = 10000    # Budget for conversion (default: 10000)
model_opus = "custom-opus-model"  # Remap opus-family model requests
model_sonnet = "custom-sonnet"    # Remap sonnet-family model requests
model_haiku = "custom-haiku"      # Remap haiku-family model requests

[[backends]]
name = "custom"
display_name = "Custom Provider"
base_url = "https://my-proxy.example.com"
auth_type = "passthrough"         # Forward original auth headers

[backends.pricing]
input_per_million = 3.00          # Cost per million input tokens
output_per_million = 15.00        # Cost per million output tokens

# Route agents to different backends
[agents]
teammate_backend = "alternative"  # Backend for teammate agents
subagent_backend = "alternative"  # Backend for subagents (optional)
```

### Authentication Types

| Type | Header | Use Case |
|------|--------|----------|
| `api_key` | `x-api-key: <value>` | Anthropic API |
| `bearer` | `Authorization: Bearer <value>` | Most providers |
| `passthrough` | Forwards original headers | OAuth flows, custom auth |

### Model Mapping

Backends can remap Anthropic model names to provider-specific ones. The proxy matches the request model against family keywords (`opus`, `sonnet`, `haiku`) and substitutes the configured name.

```toml
[[backends]]
name = "my-provider"
base_url = "https://api.example.com"
auth_type = "bearer"
api_key = "key"
model_opus = "provider-large"     # claude-opus-4-6 -> provider-large
model_sonnet = "provider-medium"  # claude-sonnet-4-5 -> provider-medium
model_haiku = "provider-small"    # claude-haiku-4-5 -> provider-small
```

Only configured families are remapped. Omitted families pass through unchanged.

Responses are automatically reverse-mapped: if the backend returns its own model name (e.g. `provider-large`), the proxy rewrites it back to the original name (e.g. `claude-opus-4-6`) so Claude Code sees a consistent model identity.

### Agent Routing

Route Claude Code's subagents and teammates to separate backends. Useful when you want the main agent on a premium provider and agents on a cheaper one.

Requires Claude Code's experimental agent teams feature. Enable it via `Ctrl+E` > Settings in the TUI.

```toml
[agents]
teammate_backend = "alternative"  # Backend for teammate agents
subagent_backend = "alternative"  # Backend for subagents (optional)
```

How it works:
- The main agent's requests go to the active backend (switchable via `Ctrl+B`)
- **Subagents** are registered via CC hooks (SubagentStart/SubagentStop) and pinned to a backend for their lifetime via session affinity. The subagent backend is also switchable via `Ctrl+B`
- **Teammates** are intercepted via a tmux shim and routed through `/teammate/{agent_id}/*` to the fixed `teammate_backend`
- Thinking block filtering is not applied to agent requests
- Backend switching does not affect agent routing

### Thinking Block Handling

AnyClaude handles two separate problems with thinking blocks when proxying through multiple backends.

#### 1. Thinking block filtering (always active)

Each provider's thinking blocks contain cryptographic signatures tied to that provider. When you switch backends mid-session, the conversation history includes thinking blocks from the previous provider. The new provider rejects these as invalid, causing 400 errors.

AnyClaude tracks all thinking blocks by content hash and automatically filters out blocks from previous sessions on backend switch. This works unconditionally for all backends — no configuration needed.

#### 2. Adaptive thinking conversion (`thinking_compat`)

Claude Code uses **adaptive thinking** — `"thinking": {"type": "adaptive"}`, where the model decides when and how much to think. The native Anthropic API supports this, but non-Anthropic backends don't. They require the explicit format: `"thinking": {"type": "enabled", "budget_tokens": N}`.

Set `thinking_compat = true` on non-Anthropic backends to enable conversion:

- **Request body:** `adaptive` -> `enabled` with a configurable token budget
- **Header:** `anthropic-beta: adaptive-thinking-*` -> `interleaved-thinking-2025-05-14`

```toml
[[backends]]
name = "alternative"
base_url = "https://your-provider.com/api"
auth_type = "bearer"
api_key = "your-api-key"
thinking_compat = true            # Convert adaptive->enabled thinking
thinking_budget_tokens = 10000    # Budget for conversion (default: 10000)
```

| Setting | Default | Description |
|---------|---------|-------------|
| `thinking_compat` | `false` | Convert adaptive thinking to explicit enabled format |
| `thinking_budget_tokens` | `10000` | Token budget for conversion. If the request has `max_tokens`, uses `max_tokens - 1` instead |

**Note:** Anthropic's own API handles adaptive thinking natively — only enable `thinking_compat` for third-party backends.

### Debug Logging

Enable detailed request/response logging for debugging:

```toml
[debug_logging]
level = "verbose"                  # "off" | "basic" | "verbose" | "full"
destination = "file"               # "stderr" | "file" | "both"
file_path = "~/.config/anyclaude/logs/debug.log"
format = "console"                 # "console" | "json"
pretty_print = true                # Pretty-print JSON bodies
full_body = false                  # Log complete bodies (no size limit)
body_preview_bytes = 1024          # Truncate preview if full_body = false
header_preview = true              # Include headers in logs

[debug_logging.rotation]
mode = "size"                      # "none" | "size" | "daily"
max_bytes = 10485760               # 10MB
max_files = 5                      # Keep 5 rotated files
```

| Level | Content |
|-------|---------|
| `off` | Disabled (default) |
| `basic` | Request timestamps, status codes, latency |
| `verbose` | + Token counts, model info, cost estimates |
| `full` | + Request/response body previews, headers |

## Development

```bash
just check              # Run lint + tests
just release 0.5.0      # Tag a release (sets version, generates changelog, creates tag)
just changelog          # Update CHANGELOG without releasing
```

```bash
./build.sh              # Build release binary (dev-stamped version)
./build.sh release-tag  # Build release without dev version stamp
./build.sh debug        # Build debug binary
./build.sh test         # Run tests
./build.sh clean        # Clean build artifacts
./build.sh install      # Build and install to ~/.cargo/bin
```

## License

Apache 2.0
