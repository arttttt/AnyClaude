# Thinking Block Compatibility Design

**Issue**: cl-db65p
**Author**: polecat/quartz
**Date**: 2026-01-31

## Overview

Enable seamless backend switching (Anthropic <-> GLM) when conversation history
contains `thinking` blocks. Providers sign these blocks differently; switching
backends can trigger "Invalid signature in thinking block" errors.

This design adds a compatibility layer that tracks which backend produced each
thinking block and transforms blocks only when they cross backend boundaries.

## Requirements

1. Three handling modes:
   - `drop_signature`: keep thinking blocks, remove signature metadata.
   - `convert_to_text`: convert thinking blocks into normal text blocks.
   - `convert_to_tags`: convert thinking blocks into text wrapped with `<think>` tags.
2. Configuration setting with default `drop_signature`.
3. Apply transformation only when a thinking block was produced by a different
   backend than the current request target.
4. Track which backend created each thinking block.
5. Log applied transformations.

## Current State

- Proxy forwards requests without JSON inspection.
- No request/response body transformation.
- Backend switching is supported via `BackendState`.

## Design Decisions

### Decision 1: Track source backend via signature map

**Choice**: Maintain an in-memory map of thinking block signatures to the backend
that produced them. If a signature is unknown during a backend switch, fall back
to the previously active backend (last request backend) as the presumed source.

**Rationale**:
- Avoids streaming response parsing in the first iteration.
- Works for steady-state usage (signatures are observed on requests while the
  same backend is active).
- Provides a safe fallback for immediate backend switches.

### Decision 2: Transform request bodies at proxy boundary

**Choice**: Parse and transform request JSON in the proxy before forwarding.

**Rationale**:
- Single choke point that sees every request.
- No changes required in Claude Code.
- Allows consistent logging and metrics.

### Decision 3: Default to drop_signature

**Choice**: Default mode is `drop_signature`.

**Rationale**:
- Preserves thinking visibility in UI.
- Minimal token impact compared to conversion.

## Configuration

Add a new top-level section:

```toml
[thinking]
mode = "drop_signature"  # or "convert_to_text" / "convert_to_tags"
```

Defaults:
- `mode = "drop_signature"`

## Data Structures

```rust
// src/proxy/thinking.rs

pub enum ThinkingMode {
    DropSignature,
    ConvertToText,
    ConvertToTags,
}

pub struct ThinkingTracker {
    mode: ThinkingMode,
    last_backend: Option<String>,
    signature_sources: LruMap<String, String>, // signature -> backend
}

pub struct ThinkingTransformResult {
    pub changed: bool,
    pub drop_count: u32,
    pub convert_count: u32,
    pub tag_count: u32,
}
```

Notes:
- `signature_sources` should be bounded (LRU or size-limited HashMap) to avoid
  unbounded growth.
- `last_backend` is updated per request (after routing decision).

## Transformation Algorithm

Given `target_backend`, JSON request body, and tracker state:

1. Parse JSON, locate `messages[*].content`.
2. For each content item with `type == "thinking"`:
   - If it has a `signature`, look up `signature_sources`.
   - If found, `source_backend = mapped backend`.
   - If not found and `last_backend != target_backend`, assume source is
     `last_backend`.
   - If not found and no switch, assume source is `target_backend`.
3. If `source_backend == target_backend`:
   - Keep block as-is.
   - Store `signature -> target_backend` if present.
4. If `source_backend != target_backend`:
   - Apply configured mode:
     - `drop_signature`: remove `signature` field, keep type `thinking`.
     - `convert_to_text`: replace with `{ "type": "text", "text": <thinking> }`.
     - `convert_to_tags`: replace with `{ "type": "text", "text": "<think>...</think>" }`.
5. Record counts for logging.

Only requests with `messages` arrays are transformed. Non-JSON or unrecognized
payloads are forwarded unchanged.

## Logging

Log transformation events at info level:

```rust
tracing::info!(
    backend = %target_backend,
    drop_count,
    convert_count,
    tag_count,
    "Applied thinking compatibility transforms"
);
```

If no changes are applied, no log is emitted (or debug level only).

## Integration Points

### Proxy

`UpstreamClient::do_forward` (or a pre-forward helper) will:
- Read request body bytes (already done).
- Apply `ThinkingTracker::transform_request` if content type is JSON.
- Forward the modified bytes.

### Router

`RouterEngine` owns a shared `ThinkingTracker` (Arc + lock) to keep state across
requests and backend switches.

### Config

Add `ThinkingConfig` to `Config` with serde defaults. Hot reload should update
the tracker mode (atomic swap or next request reads latest config).

## Choice Between Modes

- `drop_signature`: keeps the model-visible thinking blocks and avoids errors
  by removing provider signatures. Best for minimal changes and least token use.
- `convert_to_text`: turns thinking into standard text, increasing token usage
  but guaranteeing compatibility for providers that do not accept thinking blocks.
- `convert_to_tags`: keeps reasoning visible and semantically marked using
  `<think>` tags for providers that understand tagged reasoning.

## Edge Cases

- Unknown signatures immediately after a switch: treat as originating from
  `last_backend` to avoid invalid signature errors.
- Requests without `messages`: no changes.
- Multiple backends within one request: handled per block with signature map.
- Map growth: enforce max size, evict least-recently-used entries.

## Testing Strategy

1. Unit tests for JSON transformation:
   - drop_signature removes signature only when source backend differs.
   - convert_to_text replaces thinking block with text.
   - convert_to_tags wraps content with `<think>` tags.
   - same-backend blocks remain unchanged.
2. Tracker behavior:
   - signature map lookup + fallback to last_backend.
   - map eviction keeps size bounded.
3. Integration tests (proxy-level):
   - Switch A -> B -> A with thinking blocks in history does not error.
   - Mode toggles via config reload.

## Acceptance Mapping

| Criterion | Design Element |
| --- | --- |
| Mode added in config | `Config.thinking.mode` |
| Thinking blocks wrapped in `<think>` | `ThinkingMode::ConvertToTags` |
| Switches error-free | per-block source tracking + transforms |
| Documented choice between modes | "Choice Between Modes" section |

## Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| Unknown signatures during switch | fallback to last_backend |
| Large signature map | LRU or bounded map |
| JSON parse overhead | only parse when content type is JSON |
