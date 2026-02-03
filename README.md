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
- **Session Context Preservation** — Summarize conversation on backend switch (summarize mode)
- **Transparent Proxy** — Routes API requests through active backend
- **Thinking Block Handling** — Strip or summarize thinking blocks for cross-provider compatibility
- **Live Configuration** — Config hot reload on file changes
- **Image Paste** — Paste images from clipboard (Ctrl+V)
- **Debug Logging** — Request/response logging with configurable detail levels

## Architecture

```
┌─────────────────────────────┐
│     AnyClaude TUI       │
└──────────────┬──────────────┘
               │
        ┌──────▼──────┐
        │ Claude Code │
        └──────┬──────┘
               │ ANTHROPIC_BASE_URL
        ┌──────▼──────┐
        │ Local Proxy │
        │   :8080     │
        └──────┬──────┘
               │
     ┌─────────┼─────────┐
     ▼         ▼         ▼
 Backend1  Backend2   Backend3
```

## Building

```bash
cargo build --release
```

## Usage

```bash
./target/release/anyclaude
```

The wrapper automatically:
1. Starts a local proxy on `127.0.0.1:8080`
2. Sets `ANTHROPIC_BASE_URL` environment variable
3. Spawns Claude Code in an embedded terminal
4. Routes all API requests through the active backend

### Hotkeys

| Key | Action |
|-----|--------|
| `Ctrl+B` | Backend switcher popup |
| `Ctrl+S` | Status/metrics popup |
| `Ctrl+V` | Paste image from clipboard |
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
bind_addr = "127.0.0.1:8080"      # Local proxy listen address
base_url = "http://127.0.0.1:8080"

[terminal]
scrollback_lines = 10000          # History buffer size

[thinking]
mode = "summarize"                # "strip" or "summarize"

[thinking.summarize]
base_url = "https://your-summarizer-api.com"  # Anthropic-compatible API
api_key = "your-summarizer-api-key"           # API key for summarization
model = "your-model-name"                     # Model for summarization
max_tokens = 500                              # Max tokens in summary

[debug_logging]
enabled = true
level = "verbose"                 # "basic", "verbose", or "full"
path = "~/.config/anyclaude/debug.log"

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

[[backends]]
name = "custom"
display_name = "Custom Provider"
base_url = "https://my-proxy.example.com"
auth_type = "passthrough"         # Forward original auth headers
```

### Authentication Types

| Type | Header | Use Case |
|------|--------|----------|
| `api_key` | `x-api-key: <value>` | Anthropic API |
| `bearer` | `Authorization: Bearer <value>` | Most providers |
| `passthrough` | Forwards original headers | OAuth flows, custom auth |

### Thinking Block Modes

When switching between providers, thinking blocks need special handling due to signature validation:

| Mode | Behavior | Backend Switch |
|------|----------|----------------|
| `strip` | Removes all thinking blocks from requests | Instant, no context preserved |
| `summarize` | Summarizes session via external LLM on backend switch | Context preserved as summary |

**Recommended:** Use `summarize` mode for most cases — it preserves conversation context when switching backends.

#### Strip Mode

```toml
[thinking]
mode = "strip"
```

Completely removes thinking blocks from message history. Fast and stable, but loses thinking context between turns.

#### Summarize Mode (Recommended)

```toml
[thinking]
mode = "summarize"

[thinking.summarize]
base_url = "https://your-summarizer-api.com"  # Anthropic-compatible API
api_key = "your-summarizer-api-key"           # API key for summarization
model = "your-model-name"                     # Model for summarization
max_tokens = 500                              # Max tokens in summary
```

When you switch backends:
1. Current session history is summarized via the configured LLM
2. Summary is prepended to the first message on the new backend
3. New backend receives context: `[CONTEXT FROM PREVIOUS SESSION]...[/CONTEXT FROM PREVIOUS SESSION]`

This allows seamless backend switching while preserving conversation context.

### Debug Logging

Enable detailed request/response logging for debugging:

```toml
[debug_logging]
enabled = true
level = "verbose"   # "basic" | "verbose" | "full"
path = "~/.config/anyclaude/debug.log"
```

| Level | Content |
|-------|---------|
| `basic` | Request timestamps, status codes, latency |
| `verbose` | + Token counts, model info, cost estimates |
| `full` | + Request/response body previews, headers |

## License

Apache 2.0
