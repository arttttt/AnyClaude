# Target Architecture: Pluggable Composition for ClaudeWrapper

## Executive Summary

This document outlines a pragmatic refactoring plan to transform the current "god object" architecture into a pluggable, composable system. The goal is to enable autonomous features that can be added without modifying core files like `App`, `draw()`, or `runtime.rs`.

---

## 1. Gap Analysis: Current State vs Target

### 1.1 The Problem in Code

#### Current App struct (34 fields - god object)
```rust
// src/ui/app.rs:55-89
pub struct App {
    should_quit: bool,
    focus: Focus,
    size: Option<(u16, u16)>,
    pub pty_lifecycle: PtyLifecycleState,      // MVI feature
    pty_handle: Option<PtyHandle>,
    config: ConfigStore,
    error_registry: ErrorRegistry,
    ipc_sender: Option<UiCommandSender>,
    proxy_status: Option<ProxyStatus>,         // Feature: status
    metrics: Option<MetricsSnapshot>,          // Feature: metrics
    backends: Vec<BackendInfo>,               // Feature: backends
    backend_selection: usize,                 // Feature: backends
    last_ipc_error: Option<String>,
    last_status_refresh: Instant,             // Feature: status
    last_metrics_refresh: Instant,            // Feature: metrics
    last_backends_refresh: Instant,           // Feature: backends
    history_dialog: HistoryDialogState,       // MVI feature
    history_provider: Option<Arc<dyn Fn() -> Vec<HistoryEntry> + Send + Sync>>,
    settings_dialog: SettingsDialogState,     // MVI feature
    settings_manager: ClaudeSettingsManager,  // Feature: settings
    settings_saved_snapshot: HashMap<SettingId, bool>,
    pty_generation: u64,
    selection: Option<TextSelection>,
}
```

**Problem**: Adding a new feature (e.g., "Claude Code version monitor") requires modifying:
- `App` struct - add fields
- `App` methods - add accessors
- `PopupKind` enum - add variant
- `Focus` enum - already coupled to PopupKind
- `draw()` function - add rendering logic
- `classify_key()` - add hotkey handling
- `runtime.rs` - add refresh interval logic
- `run_ui_bridge()` - add command handling

#### Current draw() function (god function)
```rust
// src/ui/render.rs:22-309
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    // Header (static)
    // Terminal body (static)
    // Footer (static)

    // Popup rendering - hardcoded match on PopupKind
    if let Some(kind) = app.popup_kind() {
        if matches!(kind, PopupKind::History) {
            render_history_dialog(frame, app.history_dialog());
            return;
        }
        if matches!(kind, PopupKind::Settings) {
            render_settings_dialog(...);
            return;
        }

        // Status and BackendSwitch rendered inline
        let (title, lines) = match kind {
            PopupKind::Status => { /* 200+ lines of inline rendering */ }
            PopupKind::BackendSwitch => { /* 50+ lines of inline rendering */ }
            // Adding a new popup requires modifying this match
        };
    }
}
```

**Problem**: Closed sum type (`PopupKind` enum) requires recompilation to add variants.

#### Current input handling (hardcoded)
```rust
// src/ui/input.rs:16-68
pub fn classify_key(app: &mut App, key: &KeyInput) -> InputAction {
    // Global hotkeys - hardcoded list
    match &key.kind {
        KeyKind::Control('q') => { app.request_quit(); }
        KeyKind::Control('b') => { app.toggle_popup(PopupKind::BackendSwitch); }
        KeyKind::Control('s') => { app.toggle_popup(PopupKind::Status); }
        KeyKind::Control('h') => { app.open_history_dialog(); }
        KeyKind::Control('e') => { app.open_settings_dialog(); }
        KeyKind::Control('r') => { app.request_restart_claude(); }
        // Adding a new hotkey requires modifying this function
    }
}
```

#### Current interval-based refresh (hardcoded in runtime)
```rust
// src/ui/runtime.rs:379-393
Ok(AppEvent::Tick) => {
    app.on_tick();
    if app.should_refresh_status(STATUS_REFRESH_INTERVAL) {
        app.request_status_refresh();
    }
    if app.popup_kind() == Some(PopupKind::Status)
        && app.should_refresh_metrics(METRICS_REFRESH_INTERVAL)
    {
        app.request_metrics_refresh(None);
    }
    // Adding a new periodic refresh requires modifying the event loop
}
```

---

## 2. Target Architecture

### 2.1 Core Principle: Composition over Inheritance

Instead of features being "methods on App", features are **autonomous objects** that implement standard traits. The `App` becomes a **registry** that holds `Box<dyn Feature>` instances.

### 2.2 Key Traits

```rust
/// Lifecycle events for features
pub enum FeatureEvent {
    /// Application tick (250ms interval)
    Tick,
    /// PTY became ready
    PtyReady,
    /// Config was reloaded
    ConfigReload,
    /// Custom feature-specific event
    Custom(String),
}

/// Command that a feature can request from the runtime
pub enum FeatureCommand {
    /// Refresh data via IPC
    IpcRefresh { endpoint: &'static str },
    /// Emit an AppEvent
    EmitEvent(AppEvent),
    /// Show a popup
    ShowPopup(PopupId),
    /// Close current popup
    ClosePopup,
    /// Restart PTY
    RestartPty { env: Vec<(String, String)>, args: Vec<String> },
}

/// A pluggable UI feature
pub trait Feature: Send + 'static {
    /// Unique identifier for this feature
    fn id(&self) -> &'static str;

    /// Called during app initialization
    fn init(&mut self, ctx: &mut FeatureContext);

    /// Handle input events when this feature's popup is active
    fn handle_input(&mut self, key: &KeyInput, ctx: &mut FeatureContext) -> InputAction;

    /// Handle lifecycle events
    fn on_event(&mut self, event: FeatureEvent, ctx: &mut FeatureContext);

    /// Called on every tick, return commands to execute
    fn on_tick(&mut self, ctx: &FeatureContext) -> Vec<FeatureCommand>;

    /// Render this feature's popup content
    fn render(&self, area: Rect, ctx: &RenderContext) -> Vec<Line>;

    /// Get the popup title
    fn popup_title(&self) -> &'static str;

    /// Get footer hint text
    fn popup_footer(&self) -> &'static str;

    /// Whether this feature wants a scrollbar
    fn wants_scrollbar(&self) -> bool;
}

/// Context available to features during event handling
pub struct FeatureContext<'a> {
    pub config: &'a Config,
    pub pty_ready: bool,
    pub ipc_sender: Option<&'a UiCommandSender>,
    pub proxy_status: Option<&'a ProxyStatus>,
    // Allows features to request state from other features
    pub feature_state: &'a dyn Any,
}

/// Context available during rendering
pub struct RenderContext<'a> {
    pub app: &'a App,  // Read-only access to App state
}
```

### 2.3 Feature Registry

```rust
/// Manages all pluggable features
pub struct FeatureRegistry {
    features: HashMap<String, Box<dyn Feature>>,
    active_popup: Option<String>, // feature id
    hotkeys: HashMap<KeyCombo, String>, // key -> feature id
}

impl FeatureRegistry {
    pub fn register(&mut self, feature: Box<dyn Feature>) {
        let id = feature.id().to_string();
        self.features.insert(id, feature);
    }

    pub fn handle_key(&mut self, key: &KeyInput, ctx: &mut FeatureContext) -> InputAction {
        // Check global hotkeys first
        if let Some(feature_id) = self.hotkeys.get(&KeyCombo::from(key)) {
            self.active_popup = Some(feature_id.clone());
            return InputAction::None;
        }

        // Delegate to active feature
        if let Some(id) = &self.active_popup {
            if let Some(feature) = self.features.get_mut(id) {
                return feature.handle_input(key, ctx);
            }
        }

        InputAction::Forward
    }

    pub fn render_active_popup(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext) {
        if let Some(id) = &self.active_popup {
            if let Some(feature) = self.features.get(id) {
                let lines = feature.render(area, ctx);
                let dialog = PopupDialog::new(feature.popup_title(), lines)
                    .footer(feature.popup_footer());
                dialog.render(frame, area);
            }
        }
    }

    pub fn tick(&mut self, ctx: &FeatureContext) -> Vec<FeatureCommand> {
        let mut commands = Vec::new();
        for feature in self.features.values_mut() {
            commands.extend(feature.on_tick(ctx));
        }
        commands
    }
}
```

### 2.4 Simplified App Structure

```rust
pub struct App {
    should_quit: bool,
    focus: Focus,
    size: Option<(u16, u16)>,

    // Core PTY (always present, not pluggable)
    pty_lifecycle: PtyLifecycleState,
    pty_handle: Option<PtyHandle>,
    pty_generation: u64,

    // Shared infrastructure
    config: ConfigStore,
    error_registry: ErrorRegistry,
    ipc_sender: Option<UiCommandSender>,

    // Pluggable features
    features: FeatureRegistry,

    // Terminal-level concerns
    selection: Option<TextSelection>,
}
```

---

## 3. File-by-File Refactoring List

| File | Current Lines | Scope | Description |
|------|---------------|-------|-------------|
| `src/ui/feature.rs` (new) | ~150 | **M** | Core `Feature` trait, `FeatureContext`, `FeatureRegistry` |
| `src/ui/app.rs` | 674 | **L** | Remove feature-specific fields, integrate `FeatureRegistry` |
| `src/ui/render.rs` | 394 | **M** | Replace hardcoded popup rendering with registry delegation |
| `src/ui/input.rs` | 208 | **M** | Replace hardcoded hotkeys with registry-based dispatch |
| `src/ui/runtime.rs` | 767 | **L** | Replace hardcoded Tick handling with feature tick iteration |
| `src/ui/events.rs` | 183 | **S** | Add `FeatureEvent` variants |
| `src/ui/popup.rs` (new) | ~50 | **S** | Extract `PopupId` type, popup management traits |
| `src/features/status.rs` (new) | ~100 | **M** | Extract Status feature |
| `src/features/backends.rs` (new) | ~150 | **M** | Extract BackendSwitch feature |
| `src/features/history.rs` | existing | **S** | Adapt to `Feature` trait |
| `src/features/settings.rs` | existing | **S** | Adapt to `Feature` trait |
| `src/ui/mod.rs` | 19 | **S** | Add feature module exports |

---

## 4. Refactoring Order (Dependencies)

```
Step 1: Foundation (no dependencies)
├── Create src/ui/feature.rs with core traits
└── Add FeatureEvent variants to events.rs

Step 2: Feature Extraction (depends on Step 1)
├── Create src/features/status.rs - move Status popup logic
├── Create src/features/backends.rs - move BackendSwitch popup logic
└── Create src/features/mod.rs as registry initializer

Step 3: App Refactoring (depends on Step 1, 2)
├── Remove feature fields from App
├── Add FeatureRegistry to App
├── Update App methods to delegate to registry
└── Update PopupKind to use feature IDs

Step 4: Render Refactoring (depends on Step 3)
├── Replace draw() popup match with registry.render_active_popup()
└── Move Status/BackendSwitch inline rendering to feature render()

Step 5: Input Refactoring (depends on Step 3)
├── Replace classify_key hotkeys with registry.handle_key()
└── Move popup key handlers into respective features

Step 6: Runtime Refactoring (depends on Step 1, 3)
├── Replace hardcoded Tick intervals with registry.tick()
└── Move command dispatch to feature command processor

Step 7: MVI Features Migration (depends on Step 1)
├── Adapt History feature to Feature trait
└── Adapt Settings feature to Feature trait
```

---

## 5. Risk Assessment & Migration Strategy

### 5.1 Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Breaking existing functionality | High | Each step includes integration tests |
| `dead_code` lint violations | Medium | Remove code gradually, use `#[allow(dead_code)]` temporarily |
| Compile-time enum exhaustiveness | Low | Keep `PopupKind` during transition, migrate incrementally |
| Performance regression | Low | Benchmark before/after, FeatureRegistry uses HashMap |
| Complexity increase | Medium | Document extensively, keep trait count minimal |

### 5.2 Migration Strategy: Strangler Fig Pattern

Instead of a big-bang rewrite, use parallel implementation:

1. **Phase 1: Coexistence** (2-3 PRs)
   - Add `Feature` trait alongside existing code
   - Implement one feature (Status) as proof of concept
   - Both old and new code paths exist

2. **Phase 2: Gradual Migration** (4-6 PRs)
   - Migrate one feature per PR
   - Each PR is reviewable and testable
   - Remove old code after migration

3. **Phase 3: Cleanup** (1 PR)
   - Remove deprecated code
   - Final naming/organization polish

### 5.3 Testing Strategy

```rust
// Integration test for Feature trait
#[test]
fn test_feature_status_tick() {
    let mut status = StatusFeature::new();
    let ctx = FeatureContext::default();

    // Initially, no commands
    assert!(status.on_tick(&ctx).is_empty());

    // After interval elapsed, requests refresh
    std::thread::sleep(STATUS_INTERVAL);
    let commands = status.on_tick(&ctx);
    assert_eq!(commands.len(), 1);
    assert!(matches!(commands[0], FeatureCommand::IpcRefresh { .. }));
}
```

---

## 6. Total Effort Estimate

| Phase | Scope | Files | Est. Time |
|-------|-------|-------|-----------|
| 1. Foundation | Core traits + 1 PoC feature | 3 new, 2 modified | 4-6 hours |
| 2. Feature Extraction | Status + BackendSwitch features | 4 new, 4 modified | 6-8 hours |
| 3. App Refactoring | Integrate registry | 1 modified | 3-4 hours |
| 4. Render Refactoring | Delegate to registry | 1 modified | 2-3 hours |
| 5. Input Refactoring | Hotkey dispatch | 1 modified | 2-3 hours |
| 6. Runtime Refactoring | Tick handling | 1 modified | 3-4 hours |
| 7. MVI Migration | History + Settings | 2 modified | 4-6 hours |
| 8. Cleanup | Remove deprecated code | 5 modified | 2-3 hours |
| **Total** | | **~20 files** | **26-37 hours** |

**Recommendation**: Plan for **4-5 development days** with review cycles between phases.

---

## 7. Pragmatic Tradeoffs

### What we're NOT doing (to avoid over-engineering):

1. **No ECS** - Overkill for a TUI with ~5 features
2. **No dynamic loading** - No `dlopen`/plugins, compile-time registration only
3. **No async traits for Feature** - Keep simple sync traits, use channels for async
4. **No feature flags** - Not needed for current scope
5. **No separate crates** - Single crate, modules only

### What we ARE doing:

1. **Feature trait** - Clear contract for pluggable behavior
2. **Registry pattern** - Centralized composition point
3. **MVI where appropriate** - Dialogs with complex state (History, Settings)
4. **Simple structs where sufficient** - Status, BackendSwitch don't need full MVI
5. **Incremental migration** - Low-risk, reviewable steps

---

## 8. Example: Adding a New Feature (Post-Refactor)

```rust
// src/features/version_monitor.rs
pub struct VersionMonitorFeature {
    last_check: Instant,
    version_info: Option<VersionInfo>,
}

impl Feature for VersionMonitorFeature {
    fn id(&self) -> &'static str { "version_monitor" }

    fn popup_title(&self) -> &'static str { "Claude Code Version" }
    fn popup_footer(&self) -> &'static str { "r: Refresh  Esc: Close" }

    fn on_tick(&mut self, _ctx: &FeatureContext) -> Vec<FeatureCommand> {
        if self.last_check.elapsed() >= Duration::from_secs(300) {
            self.last_check = Instant::now();
            vec![FeatureCommand::IpcRefresh { endpoint: "/version" }]
        } else {
            vec![]
        }
    }

    fn handle_input(&mut self, key: &KeyInput, ctx: &mut FeatureContext) -> InputAction {
        match &key.kind {
            KeyKind::Char('r') => {
                ctx.command(FeatureCommand::IpcRefresh { endpoint: "/version" });
            }
            KeyKind::Escape => {
                ctx.command(FeatureCommand::ClosePopup);
            }
            _ => {}
        }
        InputAction::None
    }

    fn render(&self, _area: Rect, _ctx: &RenderContext) -> Vec<Line> {
        // Render version info...
    }
}

// In main.rs or runtime.rs:
registry.register(Box::new(VersionMonitorFeature::new()));
registry.bind_hotkey(KeyCombo::ctrl('v'), "version_monitor");

// That's it! No modifications to App, draw(), input.rs, or runtime.rs.
```
