# Claude Code ENV Injection Design

**Issue**: cl-tec.6  
**Author**: polecat/obsidian  
**Date**: 2026-01-30

## Overview

ClaudeWrapper launches Claude Code via PTY. To route Claude Code traffic through the
local proxy without touching user config files, we inject environment variables at
process start and enforce a session token on the proxy.

## Requirements Summary

1. Generate a random session token (UUID v4) at app startup.
2. Keep token in memory only (no disk persistence).
3. Inject `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN` into the Claude Code process.
4. Proxy validates `Authorization: Bearer {token}` on every request.
5. Proxy address is configurable (not hardcoded).

## Architecture

### Data Flow

```
UI runtime (startup)
  ├─ generate session_token (UUID v4)
  ├─ read proxy base_url from config
  ├─ spawn PTY with env:
  │    ANTHROPIC_BASE_URL=<proxy_base_url>
  │    ANTHROPIC_AUTH_TOKEN=<session_token>
  └─ start proxy with same session_token

Proxy router
  └─ reject request unless Authorization == "Bearer <session_token>"
```

### Configuration

Add a `proxy` section to config for routing:

```toml
[proxy]
bind_addr = "127.0.0.1:8080"
base_url = "http://127.0.0.1:8080"
```

Defaults are provided so existing configs continue to work.

## Design Decisions

### 1) Token Lifetime

Session token is created once at startup and kept in memory only. This avoids
disk persistence and ensures per-run isolation.

### 2) Token Transport

Claude Code uses `ANTHROPIC_AUTH_TOKEN` for Authorization. The proxy enforces
`Bearer <token>` on every request.

### 3) Config Surface

Separate `proxy.bind_addr` (listen address) from `proxy.base_url` (URL exposed
to Claude Code). This allows binding on `127.0.0.1` while exposing alternate
hostnames if needed.

## Implementation Plan

1. Add `ProxyConfig` to `Config` with defaults.
2. Generate UUID v4 session token at runtime startup.
3. Inject env vars when spawning Claude Code PTY.
4. Pass session token into the proxy router and enforce Authorization checks.
5. Use `proxy.bind_addr` for server bind and `proxy.base_url` for env injection.

## Acceptance Mapping

- Requests are routed to proxy via `ANTHROPIC_BASE_URL`.
- Proxy only serves requests with valid bearer token.
- Local processes without token get 401.
- No user config files are modified.
- Proxy address is config-driven.
