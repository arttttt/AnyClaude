# Backend Selector Popup (Ctrl+B)

## Goal

Provide a fast, non-disruptive popup to switch backends while Claude Code keeps running.

## User Experience

- Popup overlays the terminal body only; header and footer remain visible.
- Title: "Select Backend" in the accent color.
- Focused white border around the popup.
- Active backend is visually highlighted.
- Status column communicates Active/Ready/Missing.
- Model name is shown for each backend.
- Navigation hints at the bottom of the popup.

## Layout

The popup contents are rendered as text lines inside a bordered block.

Line layout (monospaced):

```
  <num>. <name>  [<status>]  <model>
```

Example:

```
  1. Anthropic  [Active]  claude-3-5-sonnet
  2. OpenRouter [Ready]   gpt-4o-mini
  3. Provider B      [Missing] model-name

  Up/Down: Move  Enter: Select  Esc/Ctrl+B: Close
```

## Backend Data Model

Add a model hint to the existing IPC backend payload so the UI can render model names.

- `BackendInfo`
  - `id: String`
  - `display_name: String`
  - `is_active: bool`
  - `is_configured: bool`
  - `model_hint: Option<String>`

`model_hint` uses the first model in the backend config when available.
If no models are configured, render `"unknown"`.

Status mapping:

- Active: `is_active == true`
- Ready: `is_configured == true` and not active
- Missing: `is_configured == false`

## State and Navigation

Add transient UI state to track the currently highlighted backend while the popup is open.

- `App` holds `backend_selection: usize` (0-based index)
- When opening the popup, set selection to the active backend index if found; otherwise 0.
- Arrow keys adjust selection with wraparound across the list.
- Enter triggers a backend switch via IPC, closes the popup immediately.
- Esc/Ctrl+B closes without switching.

## Rendering Rules

- Highlight the selected line with a dark surface background.
- Active backend also uses the highlight style (selected wins if different).
- Status text uses green for Active/Ready, red for Missing.
- Title uses the accent color.
- Use `Clear` behind the popup to avoid terminal artifacts.

## IPC and Backend Switching

The switch is atomic:

1. Close popup immediately on Enter.
2. Send `UiCommand::SwitchBackend` with selected backend id.
3. Header refresh should reflect the new active backend (via status refresh).

## Error Handling

- If IPC fails, show error text at the bottom of the popup on next open.
- If the backend list is empty, show a single line: "No backends available."

## Non-Goals

- No scrolling for more than 8 items (target range 2-8).
- No inline editing or config changes.
