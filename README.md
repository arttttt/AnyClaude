# ClaudeWrapper

TUI wrapper for Claude Code with hot-swappable backend support.

## Features

- **PTY Embedding** — Run Claude Code inside a terminal UI
- **Hot-Swap Backends** — Switch between Anthropic and GLM without restart
- **Local Proxy** — Reverse proxy on `localhost:4000` routes requests to active backend
- **SSE Streaming** — Full support for streaming responses
- **Configuration** — TOML-based backend configs with live reload

## Architecture

```
┌─────────────────────────────────────────────┐
│              ClaudeWrapper TUI              │
├─────────────┬─────────────┬─────────────────┤
│   Header    │   Terminal  │     Footer      │
│  (backend)  │    (PTY)    │   (hotkeys)     │
└─────────────┴──────┬──────┴─────────────────┘
                     │
              ┌──────▼──────┐
              │ Claude Code │
              │  (via PTY)  │
              └──────┬──────┘
                     │ ANTHROPIC_BASE_URL=localhost:4000
              ┌──────▼──────┐
              │ Local Proxy │
              │   :4000     │
              └──────┬──────┘
                     │
           ┌─────────┴─────────┐
           ▼                   ▼
       Anthropic              GLM
```

## Status

| Component | Status |
|-----------|--------|
| PTY & Terminal | ✅ Complete |
| UI Core Layer | ✅ Complete |
| Configuration | ✅ Complete |
| HTTP Proxy | 🚧 In Progress |
| Modal Windows | 📋 Planned |
| Integration | 📋 Planned |

## Building

```bash
cargo build --release
```

## Usage

```bash
./target/release/claudewrapper
```

### Hotkeys

- `Ctrl+B` — Switch backend
- `Ctrl+S` — Show statistics
- `Ctrl+Q` — Quit

## Configuration

Backends are configured in `~/.config/claudewrapper/backends.toml`:

```toml
[[backend]]
id = "anthropic"
name = "Anthropic"
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"

[[backend]]
id = "glm"
name = "GLM-4 (Z.AI)"
base_url = "https://api.z.ai/api/anthropic"
api_key_env = "ZAI_API_KEY"
```

## License

Apache 2.0
