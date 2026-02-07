# Thinking Blocks Architecture

This document describes how AnyClaude handles Claude's thinking blocks when proxying
requests across different backends.

## Problem Statement

Claude's thinking blocks contain cryptographic signatures that are only valid for the
backend that generated them. When a user switches backends mid-conversation:

1. Old thinking blocks have signatures from the previous backend
2. The new backend rejects these invalid signatures with a 400 error
3. The conversation becomes stuck - Claude Code keeps resending the invalid blocks

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│ Claude Code │────▶│  AnyClaude  │────▶│  Anthropic  │
│             │     │   (proxy)   │     │   Backend   │
└─────────────┘     └─────────────┘     └─────────────┘
       │                                       │
       │  Request with thinking blocks         │
       │  from GLM backend                     │
       │                                       │
       │                              400 Error│
       │◀──────────────────────────────────────│
       │  "Invalid thinking block signature"   │
```

## Solution: Session-Based Thinking Registry

We track thinking blocks by session with a confirmation lifecycle. Each backend switch
increments a session ID, invalidating all previous thinking blocks.

### Core Data Structure

```rust
struct BlockInfo {
    session: u64,           // Session when registered
    confirmed: bool,        // Seen in a request from CC
    registered_at: Instant, // When registered (for orphan cleanup)
}

pub struct ThinkingRegistry {
    current_session: u64,
    current_backend: String,
    blocks: HashMap<u64, BlockInfo>,  // content_hash → info
    orphan_threshold: Duration,       // Default: 5 minutes
}
```

### Block Lifecycle

```
┌─────────────────────────────────────────────────────────────────┐
│                     BLOCK LIFECYCLE                              │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Response arrives                                                │
│        │                                                         │
│        ▼                                                         │
│  ┌─────────────────────┐                                        │
│  │ REGISTERED          │  confirmed = false                     │
│  │ (unconfirmed)       │  registered_at = now                   │
│  └─────────────────────┘                                        │
│        │                                                         │
│        │ Block appears in next request                          │
│        ▼                                                         │
│  ┌─────────────────────┐                                        │
│  │ CONFIRMED           │  confirmed = true                      │
│  │ (in use by CC)      │                                        │
│  └─────────────────────┘                                        │
│        │                                                         │
│        │ Block disappears from request (context truncated)      │
│        ▼                                                         │
│  ┌─────────────────────┐                                        │
│  │ DELETED             │  Removed from cache                    │
│  └─────────────────────┘                                        │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Cleanup Rules

A block is removed if ANY of these conditions are true:

| Condition | Reason |
|-----------|--------|
| `session ≠ current_session` | Old session, always invalid |
| `confirmed AND ∉ request` | No longer used by CC |
| `!confirmed AND ∉ request AND age > threshold` | Orphaned block |

### Content Hashing Strategy

We use a fast hash combining:
- **Prefix**: First 256 bytes of thinking content (UTF-8 safe truncation)
- **Suffix**: Last 256 bytes of thinking content (UTF-8 safe)
- **Length**: Total content length

```rust
fn fast_hash(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    let prefix = safe_truncate(content, 256);  // UTF-8 safe
    prefix.hash(&mut hasher);
    let suffix = safe_suffix(content, 256);    // UTF-8 safe
    suffix.hash(&mut hasher);
    content.len().hash(&mut hasher);
    hasher.finish()
}
```

**Known limitation**: If two blocks have identical first 256 bytes, identical last 256 bytes, and same length, they will hash to the same value. This is acceptable for thinking blocks which rarely have this pattern.

## Request Processing Flow

The `filter_request` method is the main entry point:

```
filter_request(body):
┌─────────────────────────────────────────────────────────────────┐
│                                                                  │
│  1. EXTRACT                                                      │
│     └─▶ Get all thinking block hashes from request              │
│                                                                  │
│  2. CONFIRM                                                      │
│     └─▶ For each hash in request ∩ cache:                       │
│           └─▶ Set confirmed = true                              │
│                                                                  │
│  3. CLEANUP                                                      │
│     └─▶ Remove blocks matching cleanup rules:                   │
│           • Old session blocks                                   │
│           • Confirmed but not in request                        │
│           • Unconfirmed orphans (age > threshold)               │
│                                                                  │
│  4. FILTER                                                       │
│     └─▶ Remove thinking blocks from request body                │
│         where hash ∉ cache                                      │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Example Flow: Normal Conversation

```
Response 1 → register [A] (unconfirmed)
    ↓
Request 2 [A]
    ↓
filter_request:
  1. Confirm: A.confirmed = true
  2. Cleanup: nothing to remove
  3. Filter: A in cache → keep
    ↓
Response 2 → register [B] (unconfirmed)
    ↓
Request 3 [A, B]
    ↓
filter_request:
  1. Confirm: B.confirmed = true (A already confirmed)
  2. Cleanup: nothing to remove
  3. Filter: both in cache → keep
```

### Example Flow: Context Truncation

```
Cache: {A: confirmed, B: confirmed, C: confirmed}
    ↓
Request with [B, C] only (A was truncated)
    ↓
filter_request:
  1. Confirm: nothing new
  2. Cleanup: A is confirmed but ∉ request → DELETE
  3. Filter: B, C in cache → keep
    ↓
Cache: {B: confirmed, C: confirmed}
```

### Example Flow: Backend Switch

```
Session 1 (anthropic): Cache = {A: confirmed}
    ↓
Switch to GLM → Session 2
    ↓
Request with [A] (CC still has old block)
    ↓
filter_request:
  1. Confirm: A has session 1 ≠ current 2 → skip
  2. Cleanup: A.session ≠ current → DELETE
  3. Filter: A ∉ cache → remove from request
    ↓
Request sent without thinking blocks
    ↓
Response → register [B] (session 2, unconfirmed)
```

### Example Flow: Orphaned Block

```
Response → register [A] (unconfirmed, registered_at = T0)
    ↓
... time passes, CC never sends A ...
    ↓
Request (empty, or without A)
    ↓
filter_request:
  1. Confirm: nothing
  2. Cleanup: A is unconfirmed, ∉ request, age > threshold → DELETE
```

## Integration Points

### TransformerRegistry

The `ThinkingRegistry` is embedded in `TransformerRegistry`:

```rust
pub struct TransformerRegistry {
    current: RwLock<Arc<dyn ThinkingTransformer>>,
    config: std::sync::RwLock<ThinkingConfig>,
    thinking_registry: Mutex<ThinkingRegistry>,  // ◀── HERE
}
```

### Upstream Proxy (upstream.rs)

```rust
// Before sending request:
self.transformer_registry.notify_backend_for_thinking(&backend.name);
let filtered = self.transformer_registry.filter_thinking_blocks(&mut json_body);

// For SSE responses:
for event in sse_stream {
    self.transformer_registry.register_thinking_from_sse(data);
}

// For non-streaming responses:
self.transformer_registry.register_thinking_from_response(&body_bytes);
```

## Thinking Block Types

The registry handles two types of thinking blocks:

| Type | Content Field | Description |
|------|---------------|-------------|
| `thinking` | `thinking` | Normal thinking with visible content |
| `redacted_thinking` | `data` | Encrypted/redacted thinking |

```rust
fn extract_thinking_content(item: &Value) -> Option<String> {
    match item_type {
        "thinking" => item.get("thinking"),
        "redacted_thinking" => item.get("data"),
        _ => None,
    }
}
```

## Cache Statistics

Monitor cache state with `cache_stats()`:

```rust
pub struct CacheStats {
    pub total: usize,           // Total blocks in cache
    pub confirmed: usize,       // Blocks seen in requests
    pub unconfirmed: usize,     // Blocks not yet seen in requests
    pub current_session: usize, // Blocks from current session
    pub old_session: usize,     // Blocks from old sessions
}
```

## Why Not Filter by Signature?

Alternative approaches considered and rejected:

| Approach | Problem |
|----------|---------|
| Filter empty signatures | GLM might start generating signatures |
| Filter by signature pattern | Signatures are opaque, patterns may change |
| Strip all thinking blocks | Loses valuable context for ongoing work |
| Strip only on backend switch | Complex state management |

The session-based approach is:
- **Reliable**: Works regardless of signature format
- **Future-proof**: Doesn't depend on backend-specific behavior
- **Efficient**: O(1) lookup per thinking block
- **Self-cleaning**: Automatic cleanup based on usage patterns

## Debug Logging

The registry logs at multiple levels:

| Level | Events |
|-------|--------|
| `info` | Backend switch, request processing summary |
| `debug` | Block registration, confirmation, cleanup decisions |
| `trace` | Detailed per-block decisions |

Enable detailed logging with:

```toml
[debug_log]
enabled = true
full_body = true
pretty_print = true
```

Example log output:
```
INFO Backend switch: incremented thinking session
     old_backend="anthropic" new_backend="glm" old_session=1 new_session=2

DEBUG Registered new thinking block
      hash=12345 session=2 content_preview="Let me analyze..."

INFO Request processing complete
     confirmed=2 cleanup_old_session=1 cleanup_confirmed_unused=0
     cleanup_orphaned=0 filtered_from_request=1 cache_size_after=3
```
