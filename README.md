# ClaudeWrapper

TUI wrapper for Claude Code with hot-swappable backend support and transparent API proxying.

**Note:** Only Anthropic API-compatible backends are supported (Anthropic, GLM, and other providers that implement the Anthropic API format).

## Features

- **Hot-Swap Backends** — Switch between Anthropic, GLM, and other providers without restart
- **Transparent Proxy** — Routes API requests through active backend
- **Thinking Block Compatibility** — Transform thinking blocks between provider formats
- **Live Configuration** — Config hot reload on file changes
- **Image Paste** — Paste images from clipboard
- **Metrics** — Request latency, error tracking (Ctrl+S)

## Architecture

```
┌─────────────────────────────┐
│     ClaudeWrapper TUI       │
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
 Anthropic    GLM      Other
```

## Building

```bash
cargo build --release
```

## Usage

```bash
./target/release/claudewrapper
```

The wrapper automatically:
1. Starts a local proxy on `127.0.0.1:8080`
2. Sets `ANTHROPIC_BASE_URL` environment variable
3. Spawns Claude Code in an embedded terminal
4. Routes all API requests through the active backend

### Hotkeys

- `Ctrl+B` — Backend switcher
- `Ctrl+S` — Status/metrics popup
- `Ctrl+Q` — Quit
- `1-9` — Quick-select backend (in switcher)

## Configuration

Config location: `~/.config/claude-wrapper/config.toml`

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
mode = "drop_signature"           # See "Thinking Block Modes" below

[[backends]]
name = "anthropic"
display_name = "Anthropic"
base_url = "https://api.anthropic.com"
auth_type = "api_key"
api_key_env = "ANTHROPIC_API_KEY"

[[backends]]
name = "glm"
display_name = "GLM-4 (Z.AI)"
base_url = "https://open.bigmodel.cn/api/paas/v4"
auth_type = "bearer"
api_key_env = "GLM_API_KEY"

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
| `bearer` | `Authorization: Bearer <value>` | Most providers (GLM, OpenAI-compatible) |
| `passthrough` | Forwards original headers | OAuth flows, custom auth |

API keys can be specified directly (`api_key = "sk-..."`) or via environment variable (`api_key_env = "ENV_VAR_NAME"`).

### Thinking Block Modes

When switching between providers, thinking blocks may need transformation:

| Mode | Behavior |
|------|----------|
| `drop_signature` | Removes provider-specific signature, keeps thinking block structure |
| `convert_to_text` | Converts thinking blocks to plain text content |
| `convert_to_tags` | Wraps thinking content in `<think>...</think>` tags |

## License

Apache 2.0
