# Network Diagnostics Popup (Ctrl+S)

## Goal

Provide a read-only telemetry panel for diagnosing network issues. Helps users understand whether slowness is due to network or model thinking.

## User Experience

- Popup overlays the terminal body only; header and footer remain visible.
- Title: "Network Diagnostics" in Claude Orange (`CLAUDE_ORANGE`).
- Focused white border around the popup (`POPUP_BORDER`).
- Read-only panel - no keyboard navigation needed.
- Status indicators: green for connected/healthy, red for errors.

## Layout

The popup contents are rendered as labeled rows inside a bordered block.

Field layout (monospaced, label-value pairs):

```
  Provider:  Claude API
  Model:     claude-sonnet-4-20250514
  URL:       https://api.anthropic.com
  Status:    Connected   ●
  Latency:   142 ms
  Tokens:    1,234 in / 567 out

  Esc/Ctrl+S: Close
```

## Display Fields

1. **Provider** - Backend display name (e.g., "Claude API", "Provider B")
   - Source: `BackendInfo.display_name` for the active backend

2. **Model** - Current model identifier
   - Source: `BackendInfo.model_hint` for the active backend
   - Fallback: "unknown" if not configured

3. **URL** - API endpoint (truncated if too long)
   - Source: `BackendInfo.base_url` (new field to add)
   - Truncate to 40 characters with "..." if longer

4. **Status** - Connection state with colored indicator
   - Source: `ProxyStatus.healthy` and recent request success
   - Values: "Connected" (green ●) or "Error" (red ●)

5. **Latency** - Round-trip time in milliseconds
   - Source: Most recent `RequestRecord.latency_ms` for active backend
   - Fallback: "—" if no requests yet
   - Format: "{value} ms"

6. **Tokens** - Input/Output token counts from last request
   - Source: `RequestAnalysis.estimated_input_tokens` (input estimate)
   - Source: Parse response for usage data (output) or "—" if unavailable
   - Format: "{in} in / {out} out"

## Data Model Changes

### BackendInfo Extension

Add `base_url` field to `BackendInfo` struct in `src/ipc/mod.rs`:

```rust
#[derive(Debug, Clone)]
pub struct BackendInfo {
    pub id: String,
    pub display_name: String,
    pub is_active: bool,
    pub is_configured: bool,
    pub model_hint: Option<String>,
    pub base_url: String,  // NEW: API endpoint URL
}
```

Update `IpcCommand::ListBackends` handler to populate `base_url` from backend config.

## State and Keyboard

- `PopupKind::Status` becomes `PopupKind::NetworkDiagnostics`
- Ctrl+S: Toggle popup open/close
- Escape: Close popup
- No navigation keys needed (read-only display)

## Rendering

Following the same pattern as BackendSwitch popup:

1. Build lines vector with styled spans
2. Use consistent label width for alignment (10 characters: "Provider: ")
3. Apply `STATUS_OK` (green) or `STATUS_ERROR` (red) for status indicator
4. Calculate content width from lines, add padding
5. Center popup using `centered_rect_by_size`
6. Render with `Clear`, `Block`, and `Paragraph`

### Styling Rules

- Labels: `HEADER_TEXT` color
- Values: `HEADER_TEXT` color
- Status "Connected": `STATUS_OK` color
- Status "Error": `STATUS_ERROR` color
- Status indicator (●): Same color as status text
- Help line: `HEADER_TEXT` color, dimmed

## Data Flow

1. On popup open, refresh metrics via `RefreshMetrics` IPC command
2. Read from `app.proxy_status()`, `app.backends()`, `app.metrics()`
3. Find active backend from backends list
4. Get most recent request record for that backend from metrics

## Error Handling

- If no active backend: show "No backend configured"
- If no requests yet: show "—" for latency and tokens
- If IPC error: show error message at bottom of popup

## Implementation Steps

1. Add `base_url` to `BackendInfo` struct
2. Update `ListBackends` IPC handler to include `base_url`
3. Rename `PopupKind::Status` to `PopupKind::NetworkDiagnostics`
4. Update render.rs to show new layout with 6 fields
5. Update input.rs comments (Ctrl+S description)
6. Update footer hints if needed

## Non-Goals

- Live auto-refresh while popup is open (future enhancement)
- Scrollable history of requests
- Editing network configuration
