# Subagent Session Affinity via CC Hooks

## Problem

`SubagentBackend` (`Arc<RwLock<Option<String>>>`) читается `detect_marker_model()` на **каждый** запрос. Субагент CC делает несколько API-запросов за сессию (tool use → ответ → tool result → следующий). Если пользователь сменил бэкенд через UI между запросами — субагент переключится на другой бэкенд mid-session.

**Требование:** субагент должен всё время своей жизни работать с тем бэкендом, на котором запустился.

## Solution Overview

Используем CC hooks (`SubagentStart` / `SubagentStop`) для инжекции имени бэкенда прямо в контекст субагента через `additionalContext`. Хуки инжектируются в рантайме через CLI флаг `--settings` — без модификации файлов пользователя.

```
┌──────────────────────────────────────────────────────────────────┐
│                         AnyClaude                                │
│                                                                  │
│  ArgAssembler: --settings '{"hooks": {                           │
│    "SubagentStart": [{ curl → POST /api/subagent-start }],      │
│    "SubagentStop":  [{ curl → POST /api/subagent-stop  }]       │
│  }}'                                                             │
│                                                                  │
│  ┌──────────────────┐    ┌──────────────────────────────┐        │
│  │  Claude Code      │    │  Proxy                       │        │
│  │                   │    │                              │        │
│  │  1. Spawn subagent│    │  /api/subagent-start:        │        │
│  │     → hook fires  │───→│    record agent → backend    │        │
│  │     ← additional  │←───│    return additionalContext  │        │
│  │       Context     │    │    "⟨AC:backend_name⟩"       │        │
│  │                   │    │                              │        │
│  │  2. Subagent req  │    │  detect_marker_model():      │        │
│  │     model:        │───→│    extract "⟨AC:backend⟩"    │        │
│  │     anyclaude-    │    │    from request body         │        │
│  │     subagent      │    │    → route to that backend   │        │
│  │                   │    │                              │        │
│  │  3. Subagent done │    │  /api/subagent-stop:         │        │
│  │     → hook fires  │───→│    cleanup record            │        │
│  └──────────────────┘    └──────────────────────────────┘        │
└──────────────────────────────────────────────────────────────────┘
```

## Detailed Design

### 1. Hook Injection via `--settings`

CC поддерживает `--settings <file-or-json>` — дополнительные settings, которые **мержатся** с пользовательскими. Файлы пользователя не затрагиваются.

Используем **command** тип хуков с `curl`. Команда получает hook data через stdin и отправляет на прокси. Stdout команды парсится CC как JSON-ответ (для `additionalContext`).

**Генерация JSON:**
```rust
// src/args/assembler.rs
pub fn with_subagent_hooks(mut self, proxy_port: u16) -> Self {
    let hooks_json = format!(
        r#"{{"hooks":{{"SubagentStart":[{{"matcher":"","hooks":[{{"type":"command","command":"curl -s -X POST http://127.0.0.1:{port}/api/subagent-start -d @- -H 'Content-Type: application/json'"}}]}}],"SubagentStop":[{{"matcher":"","hooks":[{{"type":"command","command":"curl -s -X POST http://127.0.0.1:{port}/api/subagent-stop -d @- -H 'Content-Type: application/json'"}}]}}]}}}}"#,
        port = proxy_port
    );
    self.args.push("--settings".into());
    self.args.push(hooks_json);
    self
}
```

**Вызов в pipeline.rs:**
```rust
let args = ArgAssembler::from_passthrough(&classified)
    .with_session(&session, mode)
    .with_settings(&settings)
    .with_teammate_mode(shim)
    .with_subagent_hooks(proxy_port)  // NEW
    .with_extra(extra_args)
    .build();
```

### 2. Proxy API Endpoints

**File:** `src/proxy/router.rs` — два новых route

#### POST /api/subagent-start

**Request** (from CC hook, stdin → curl → proxy):
```json
{
  "session_id": "abc123",
  "hook_event_name": "SubagentStart",
  "agent_name": "researcher",
  "agent_type": "Explore"
}
```

**Handler:**
1. Читает текущий `SubagentBackend` state
2. Если `None` → возвращает пустой ответ (субагент роутится дефолтно)
3. Если `Some(backend)` → возвращает `additionalContext` с маркером

**Response** (→ curl stdout → CC parses):
```json
{
  "hookSpecificOutput": {
    "hookEventName": "SubagentStart",
    "additionalContext": "⟨AC:openrouter⟩"
  }
}
```

CC инжектит `additionalContext` как `<system-reminder>` в message stream субагента.

#### POST /api/subagent-stop

**Request:**
```json
{
  "session_id": "abc123",
  "hook_event_name": "SubagentStop",
  "agent_name": "researcher",
  "agent_type": "Explore"
}
```

**Handler:** Логирование. Никакого состояния чистить не нужно — бэкенд закодирован в маркере самого субагента.

**Response:** `200 OK` (пустой body).

### 3. Routing: extract marker from request body

**File:** `src/proxy/pipeline/routing.rs`

В `resolve_backend()`, AC marker extraction happens before marker model detection:

```rust
pub fn resolve_backend(
    backend_state: &BackendState,
    _subagent_backend: &SubagentBackend,
    backend_override: Option<String>,
    plugin_override: Option<BackendOverride>,
    parsed_body: Option<&Value>,
    ctx: &mut PipelineContext,
) -> Result<Backend, ProxyError> {
    let active_backend = backend_state.get_active_backend();

    // 1. Extract AC marker from request body (session affinity from hook)
    let ac_marker_backend = parsed_body.and_then(extract_ac_marker);

    // 2. Check for marker model prefixes (marker-*, anyclaude-*) or direct backend name
    let marker_backend = parsed_body
        .and_then(|body| body.get("model"))
        .and_then(|m| m.as_str())
        .and_then(|model| detect_marker_model(model, backend_state));

    // Priority: plugin_override > backend_override > ac_marker_backend > marker_backend > active_backend
    let backend_id = plugin_override
        .as_ref()
        .map(|o| o.backend.clone())
        .or(backend_override.clone())
        .or(ac_marker_backend.clone())
        .or(marker_backend.clone())
        .unwrap_or(active_backend);
    // ...
}
```

**Extraction function:**
```rust
/// Extract "⟨AC:backend_name⟩" marker from request body.
/// Searches in system field and messages content.
fn extract_ac_marker(body: &serde_json::Value) -> Option<String> {
    let body_str = body.to_string();
    // Simple substring search for the marker
    let start = body_str.find("⟨AC:")?;
    let rest = &body_str[start + "⟨AC:".len()..];
    let end = rest.find('⟩')?;
    let backend = &rest[..end];
    // Validate: non-empty, no special chars
    if !backend.is_empty() && backend.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        Some(backend.to_string())
    } else {
        None
    }
}
```

**Почему Unicode маркеры `⟨` `⟩`:** минимизируют коллизии с обычным текстом. `<AC:...>` мог бы конфликтовать с XML/HTML в контенте.

### 4. Marker Design

**Format:** `⟨AC:backend_name⟩`

- `⟨` (U+27E8, Mathematical Left Angle Bracket)
- `AC` — AnyClaude prefix
- `backend_name` — имя бэкенда из конфига
- `⟩` (U+27E9, Mathematical Right Angle Bracket)

**Пример:** `⟨AC:openrouter⟩`

**Свойства:**
- Уникальный формат — не встречается в обычном тексте
- Короткий — не раздувает контекст
- Содержит имя бэкенда напрямую — не нужен lookup в registry
- Если бэкенд переименован/удалён → fallback на текущий SubagentBackend state

### 5. Fallback Strategy

| Ситуация | Поведение |
|----------|-----------|
| Маркер `⟨AC:X⟩` найден, бэкенд X существует | → Route to X |
| Маркер найден, бэкенд X не существует | → Fallback to marker model, then active backend |
| Маркер не найден (hooks не сработали) | → Check marker-* prefix or direct backend name |
| Ничего не найдено | → Default routing (active backend) |

Priority: `ac_marker_backend > marker_backend > active_backend`. Маркер обеспечивает session affinity — субагент остаётся на том же бэкенде всю сессию.

---

## Changes by File

### 1. ArgAssembler: hook injection

**File:** `src/args/assembler.rs`

```rust
/// Inject SubagentStart/SubagentStop hooks via --settings CLI flag.
/// CC merges these with user settings. No user files are modified.
pub fn with_subagent_hooks(mut self, proxy_port: u16) -> Self {
    let hooks_json = format!(
        r#"{{"hooks":{{"SubagentStart":[{{"matcher":"","hooks":[{{"type":"command","command":"curl -s -X POST http://127.0.0.1:{port}/api/subagent-start -d @- -H 'Content-Type: application/json'"}}]}}],"SubagentStop":[{{"matcher":"","hooks":[{{"type":"command","command":"curl -s -X POST http://127.0.0.1:{port}/api/subagent-stop -d @- -H 'Content-Type: application/json'"}}]}}]}}}}"#,
        port = proxy_port
    );
    self.args.push("--settings".into());
    self.args.push(hooks_json);
    self
}
```

### 2. Pipeline: pass proxy_port

**File:** `src/args/pipeline.rs`

Add `proxy_port: u16` parameter to `build_spawn_params` and `build_restart_params`.

Wire through to `ArgAssembler::with_subagent_hooks(proxy_port)`.

### 3. Proxy Router: hook endpoints

**File:** `src/proxy/router.rs`

Two new routes:
- `POST /api/subagent-start` → `handle_subagent_start()`
- `POST /api/subagent-stop` → `handle_subagent_stop()`

### 4. Hook Handlers

**File:** `src/proxy/hooks.rs` (new file)

```rust
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use crate::backend::state::SubagentBackend;

#[derive(Deserialize)]
pub struct SubagentHookInput {
    pub session_id: Option<String>,
    pub hook_event_name: Option<String>,
    pub agent_name: Option<String>,
    pub agent_type: Option<String>,
}

#[derive(Serialize)]
pub struct HookSpecificOutput {
    #[serde(rename = "hookEventName")]
    pub hook_event_name: String,
    #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
}

#[derive(Serialize)]
pub struct SubagentStartResponse {
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: HookSpecificOutput,
}

pub async fn handle_subagent_start(
    State(subagent_backend): State<SubagentBackend>,
    Json(input): Json<SubagentHookInput>,
) -> Json<SubagentStartResponse> {
    let context = subagent_backend.get().map(|backend| {
        format!("⟨AC:{}⟩", backend)
    });

    Json(SubagentStartResponse {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "SubagentStart".into(),
            additional_context: context,
        },
    })
}

pub async fn handle_subagent_stop(
    Json(input): Json<SubagentHookInput>,
) -> axum::http::StatusCode {
    // Log if needed, no state to clean up
    axum::http::StatusCode::OK
}
```

### 5. Routing: marker extraction

**File:** `src/proxy/pipeline/routing.rs`

- Add `extract_ac_marker(body: &Value) -> Option<String>`
- Update `resolve_backend()` to extract AC marker from body before marker model detection
- Priority: `plugin_override > backend_override > ac_marker_backend > marker_backend > active_backend`

### 6. Runtime: pass proxy_port

**File:** `src/ui/runtime.rs`

Extract proxy port and pass to `build_spawn_params()` / `build_restart_params()`.

---

## Files That Do NOT Need Changes

| File | Reason |
|------|--------|
| `src/backend/state.rs` | SubagentBackend unchanged — still used as fallback |
| `src/proxy/pipeline/transform.rs` | model_map still works the same |
| `src/proxy/pipeline/headers.rs` | Auth headers unchanged |
| `src/proxy/pipeline/forward.rs` | Forwarding unchanged |
| `src/config/types.rs` | No new config fields |
| `~/.claude/settings.json` | NOT modified — hooks injected via --settings |

---

## Verification

1. **Hook injection:** start AnyClaude → debug log shows `--settings` with hooks JSON in CLI args
2. **Hook fires:** create subagent (Task tool) → proxy log shows `POST /api/subagent-start`
3. **Marker injection:** subagent's first request contains `⟨AC:openrouter⟩` in body
4. **Session affinity:** change subagent backend via UI → existing subagent continues on old backend
5. **New subagent:** after backend change, new subagent gets new backend via marker
6. **Fallback:** disable hooks → subagent uses SubagentBackend state (no marker)
7. **No hooks fallback:** SubagentBackend = None → default routing
8. **Context compression:** long-running subagent triggers compaction → marker survives in <system-reminder>
9. **Tests:** `cargo test` — all tests pass

---

## Limitations and Edge Cases

1. **`additionalContext` and compression** — маркер инжектируется как `<system-reminder>`, который CC обычно сохраняет при сжатии. Если потеряется → graceful fallback на SubagentBackend state.

2. **Enterprise `allowManagedHooksOnly`** — корпоративная настройка может заблокировать инжекцию хуков. В этом случае session affinity не работает, но SubagentBackend fallback обеспечивает базовую функциональность.

3. **curl dependency** — хук использует curl для HTTP POST. На macOS и Linux curl предустановлен. На Windows может потребоваться альтернатива.

4. **Pane-based teammates** — SubagentStart/SubagentStop НЕ срабатывают для pane-based (tmux) teammates. Но нам это не нужно — тиммейты роутятся через BackendOverride, а их субагенты используют дефолтный роутинг.

5. **Concurrent subagents** — несколько субагентов одновременно: каждый получает свой маркер с бэкендом на момент запуска. Если бэкенд изменён между запусками → разные субагенты на разных бэкендах. Это корректное поведение.

6. **Backend renamed/deleted** — если бэкенд из маркера больше не существует → fallback на SubagentBackend state → fallback на active backend.

7. **`--settings` merge** — CC мержит settings, а не заменяет. Если у пользователя уже есть SubagentStart hook, оба хука выполнятся. Порядок выполнения определяется CC.
