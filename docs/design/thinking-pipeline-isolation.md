# Изоляция Thinking Pipeline для мульти-агентных сессий

**Дата**: 2026-02-12
**Статус**: Проект
**Связан с**: [Agent Team Routing](agent-team-routing.md), [Thinking Blocks Architecture](../architecture/thinking-blocks.md)

## Проблема

ThinkingRegistry спроектирован для одного агента. При работе с Agent Teams
(main agent + N teammates) через один прокси возникают конфликты:

### Глобальный session counter

`ThinkingRegistry` содержит один `current_session: u64`, который инкрементируется
при каждой смене бэкенда (`on_backend_switch`). Запросы main agent (kimi) и
teammate (glm) чередуются → каждое чередование инкрементирует сессию:

```
Main agent (kimi):     notify("kimi")  → session 1
Teammate   (glm):      notify("glm")   → session 2  (switch!)
Main agent (kimi):     notify("kimi")  → session 3  (switch!)
Teammate   (glm):      notify("glm")   → session 4  (switch!)
```

Блоки, зарегистрированные main agent в session 1, становятся невалидными
в session 3. При следующем запросе main agent `filter_request()` удаляет
ВСЕ его thinking блоки как "old session".

### Общий block cache

Один `blocks: HashMap<u64, BlockInfo>` на всех агентов. Cleanup от одного
агента удаляет блоки другого. Teammate-блоки и main-блоки смешиваются.

### Race между notify и capture

`notify_backend_for_thinking()` и `current_thinking_session()` — два
отдельных lock acquisition. Между ними другой агент может инкрементировать
сессию:

```rust
// Thread A (main agent)
self.transformer_registry.notify_backend_for_thinking("kimi");  // lock, unlock
// ← Thread B (teammate): notify("glm") → session++
let session_id = self.transformer_registry.current_thinking_session();  // lock, unlock — WRONG session!
```

### Почему это критично именно для Agent Teams

До Agent Teams прокси обслуживал одного агента. Запросы были последовательными.
Backend switch происходил редко (ручное переключение через Ctrl+B).

С Agent Teams запросы конкурентные. Main agent и teammates работают
параллельно. Backend "переключается" на каждом запросе, потому что разные
агенты используют разные бэкенды.

## Анализ

### Что реально нужно тиммейтам от thinking pipeline

Тиммейты используют фиксированный бэкенд (`teammate_backend`), который
**никогда не переключается**. Это значит:

- `on_backend_switch()` — не нужен (бэкенд не меняется)
- `filter_request()` — не нужен (нет невалидных блоков от другого бэкенда)
- `register_block()` — не нужен (блоки не нужно отслеживать)

Тиммейтам **вообще не нужен ThinkingRegistry**. Он решает проблему
переключения бэкенда, которой у тиммейтов нет.

### Что нужно всем запросам (и main, и teammate)

Два body transform, которые **не зависят** от ThinkingRegistry:

| Transform | Назначение | Зависимость |
|-----------|-----------|-------------|
| `apply_model_map` | Переписать model field по family mapping | `Backend` config |
| `apply_thinking_compat` | `adaptive` → `enabled` + budget | `Backend` config |

Эти transform применяются **всегда**, для всех запросов.

### Что нужно только main agent

Thinking lifecycle — полный цикл управления thinking блоками:

| Фаза | Когда | Что делает |
|------|-------|-----------|
| `begin_request` | До отправки | `notify_backend` + capture `session_id` (атомарно) |
| `filter` | Body transform | Удалить невалидные блоки из тела запроса |
| `register` | После ответа | Зарегистрировать новые блоки из response/SSE |

## Текущая архитектура

```
Router
  └── routing_middleware (if rules exist)
        └── proxy_handler
              └── forward()
                    └── do_forward()
                          ├── notify_backend()        ← thinking
                          ├── capture session_id       ← thinking
                          ├── apply_model_map()        ← body transform
                          ├── apply_thinking_compat()  ← body transform
                          ├── filter_thinking_blocks() ← thinking
                          ├── NativeTransformer        ← dead code (no-op)
                          ├── send + retry
                          ├── SSE callback: register() ← thinking
                          └── non-stream: register()   ← thinking
```

Проблемы текущей архитектуры:

1. **Всё в одном монолите** — `do_forward()` ~500 строк, смешивает HTTP,
   body transforms, thinking lifecycle, debug logging
2. **Thinking применяется безусловно** — каждый запрос проходит через
   `notify_backend`, `filter`, `register`, даже тиммейты которым это не нужно
3. **TransformerRegistry на UpstreamClient** — HTTP-клиент владеет thinking
   бизнес-логикой, нарушая single responsibility
4. **Dead code** — `NativeTransformer` (no-op), `TransformContext`,
   `TransformResult`, `TransformError` — не делают ничего полезного

## Решение: композиция пайплайнов на уровне роутера

Ключевая идея: **разные маршруты — разные пайплайны**. Не один пайплайн
с runtime-проверками, а два пайплайна, собранные при старте. Каждый содержит
только нужные middleware. Никакой middleware не зависит от результата другого.

### Целевая архитектура

```
Router
├── /health
│     └── health_handler
│
├── /teammate/*                              ← пайплайн БЕЗ thinking
│     ├── Extension(BackendOverride("glm"))  ← конфигурация, не middleware result
│     └── proxy_handler
│           └── forward() → do_forward()
│                 ├── apply_model_map()
│                 ├── apply_thinking_compat()
│                 ├── send + retry
│                 └── response (без register)
│
└── /* (fallback)                            ← пайплайн С thinking
      ├── thinking_middleware                ← создаёт ThinkingSession
      └── proxy_handler
            └── forward() → do_forward()
                  ├── extract ThinkingSession from extensions
                  ├── apply_model_map()
                  ├── apply_thinking_compat()
                  ├── session.filter()
                  ├── send + retry
                  ├── SSE: session.register_from_sse()
                  └── non-stream: session.register_from_response()
```

### Почему не один middleware с runtime-проверкой

Если `thinking_middleware` проверяет `RoutedTo` из extensions — он **зависит**
от `routing_middleware`. Это coupling между middleware, порядок их применения
становится важен, и изменение одного может сломать другой.

Вместо этого: `axum::Router::nest()` создаёт отдельный пайплайн для
`/teammate/*`. Axum нативно strip'ит префикс — кастомный `routing_middleware`
не нужен. Каждый пайплайн содержит только свои middleware.

### Компоненты

#### 1. `ThinkingSession` (новый struct)

Per-request handle для thinking lifecycle. Создаётся в middleware,
потребляется в handler.

```rust
// src/proxy/thinking/mod.rs

pub struct ThinkingSession {
    registry: Arc<TransformerRegistry>,
    session_id: u64,
    debug_logger: Arc<DebugLogger>,
}

impl ThinkingSession {
    /// Фильтрация невалидных thinking блоков из request body.
    pub fn filter(&self, body: &mut serde_json::Value) -> u32 { ... }

    /// Регистрация блоков из SSE stream (вызывается в on_complete callback).
    pub fn register_from_sse(&self, events: &[SseEvent]) { ... }

    /// Регистрация блоков из non-streaming response.
    pub fn register_from_response(&self, body: &[u8]) { ... }
}
```

Методы делегируют в `TransformerRegistry`, передавая `self.session_id`.
Session ID зафиксирован при создании — race condition с конкурентными
запросами устранён.

#### 2. `TransformerRegistry::begin_request()` (новый метод)

Атомарно выполняет `notify + capture` в одном lock acquisition:

```rust
impl TransformerRegistry {
    pub fn begin_request(
        self: &Arc<Self>,
        backend: &str,
        debug_logger: Arc<DebugLogger>,
    ) -> ThinkingSession {
        let mut reg = self.thinking_registry.lock();
        reg.on_backend_switch(backend);
        let session_id = reg.current_session();
        ThinkingSession {
            registry: Arc::clone(self),
            session_id,
            debug_logger,
        }
    }
}
```

Раньше `notify` и `capture` были двумя отдельными lock — между ними
другой поток мог изменить session. Теперь это атомарная операция.

#### 3. `BackendOverride` (новый тип, заменяет RoutedTo)

```rust
// src/proxy/routing.rs

/// Фиксированный бэкенд, заданный при сборке роутера.
/// Ставится как Extension на вложенный роутер, не как результат middleware.
#[derive(Clone)]
pub struct BackendOverride(pub String);
```

`RoutedTo` удаляется — он был результатом middleware-evaluation.
`BackendOverride` — конфигурация, заданная при сборке роутера.

#### 4. `thinking_middleware` (новый)

```rust
// src/proxy/router.rs

async fn thinking_middleware(
    State(state): State<RouterEngine>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let backend = state.backend_state.get_active_backend();
    let session = state.transformer_registry.begin_request(
        &backend,
        state.debug_logger.clone(),
    );
    req.extensions_mut().insert(session);
    next.run(req).await
}
```

Безусловный — вызывается только в main-пайплайне. Не проверяет
чужие extensions. Не знает про routing.

#### 5. `build_router()` (переделка)

```rust
pub fn build_router(
    engine: RouterEngine,
    teams: &Option<AgentTeamsConfig>,
) -> Router {
    // Main pipeline: body transforms + thinking lifecycle
    let main = Router::new()
        .fallback(proxy_handler)
        .layer(axum::middleware::from_fn_with_state(
            engine.clone(),
            thinking_middleware,
        ))
        .with_state(engine.clone());

    let mut router = Router::new()
        .route("/health", get(health_handler))
        .with_state(engine.clone());

    // Teammate pipeline: body transforms only, no thinking
    if let Some(config) = teams {
        let teammate = Router::new()
            .fallback(proxy_handler)
            .layer(Extension(BackendOverride(
                config.teammate_backend.clone(),
            )))
            .with_state(engine.clone());

        router = router.nest("/teammate", teammate);
    }

    router.merge(main)
}
```

Axum `nest("/teammate", ...)` автоматически strip'ит `/teammate`
из URI — кастомный `rewrite_uri()` не нужен.

#### 6. `proxy_handler` (изменение)

```rust
async fn proxy_handler(
    State(state): State<RouterEngine>,
    RawQuery(query): RawQuery,
    req: Request<Body>,
) -> Response {
    // Backend: from BackendOverride (teammate) or active backend (main)
    let backend_override = req.extensions()
        .get::<BackendOverride>()
        .map(|bo| bo.0.clone());

    // ... forward with backend_override ...
}
```

#### 7. `do_forward()` (изменение)

Извлекает `ThinkingSession` из extensions. Если есть — применяет
filter и register. Если нет — пропускает. Без флагов, без проверки
типа запроса.

```rust
async fn do_forward(&self, req: Request<Body>, ...) {
    let (mut parts, body) = req.into_parts();
    let thinking = parts.extensions.remove::<ThinkingSession>();

    // Body transform pipeline
    // ...parse JSON...
    apply_model_map(&mut json_body, &backend, &self.debug_logger);
    apply_thinking_compat(&mut json_body, &backend, &self.debug_logger);
    if let Some(ref session) = thinking {
        session.filter(&mut json_body);
    }

    // ... headers, send, retry ...

    // Response handling
    if is_streaming {
        if let Some(session) = thinking {
            let on_complete = Box::new(move |bytes| {
                let events = parse_sse_events(bytes);
                session.register_from_sse(&events);
            });
            observed = observed.with_on_complete(on_complete);
        }
    } else {
        if let Some(ref session) = thinking {
            session.register_from_response(&body_bytes);
        }
    }
}
```

### Ownership / DI

```
RouterEngine (axum State, Clone)
├── backend_state: BackendState
├── transformer_registry: Arc<TransformerRegistry>   ← ПЕРЕЕЗЖАЕТ из UpstreamClient
├── upstream: Arc<UpstreamClient>                    ← теряет registry
├── observability: ObservabilityHub
├── debug_logger: Arc<DebugLogger>
└── health: Arc<HealthHandler>

UpstreamClient (чистый HTTP forwarder)
├── client: reqwest::Client
├── timeout_config: TimeoutConfig
├── pool_config: PoolConfig
├── debug_logger: Arc<DebugLogger>
├── request_parser: RequestParser
└── response_parser: ResponseParser

ThinkingSession (per-request, в extensions)
├── registry: Arc<TransformerRegistry>
├── session_id: u64
├── debug_logger: Arc<DebugLogger>
└── methods: filter(), register_from_sse(), register_from_response()
```

`UpstreamClient` перестаёт владеть `TransformerRegistry`. Он не знает
про thinking — только форвардит HTTP. Thinking lifecycle управляется
на уровне роутера (middleware + extensions).

## Request flow

### Main agent: `POST /v1/messages`

```
1. axum router: fallback match → main pipeline
2. thinking_middleware:
   - backend = backend_state.get_active_backend()  // "kimi"
   - session = registry.begin_request("kimi")       // atomic notify+capture
   - extensions.insert(session)
3. proxy_handler:
   - no BackendOverride → backend_override = None
   - forward(req, backend_state, None, ...)
4. forward():
   - backend = backend_state.get_active_backend_config()  // kimi config
   - do_forward(req, backend, ...)
5. do_forward():
   - thinking = extensions.remove::<ThinkingSession>()  // Some(session)
   - apply_model_map      → noop (kimi uses native models)
   - apply_thinking_compat → noop (kimi is Anthropic)
   - session.filter()      → remove invalid thinking blocks
   - send to kimi
   - SSE callback: session.register_from_sse()
```

### Teammate: `POST /teammate/v1/messages`

```
1. axum router: nest("/teammate") match → teammate pipeline
   axum strips "/teammate" → URI becomes /v1/messages
2. NO thinking_middleware (not in this pipeline)
3. proxy_handler:
   - BackendOverride("glm") → backend_override = Some("glm")
   - forward(req, backend_state, Some("glm"), ...)
4. forward():
   - backend = backend_state.get_backend_config("glm")  // glm config
   - do_forward(req, backend, ...)
5. do_forward():
   - thinking = extensions.remove::<ThinkingSession>()  // None
   - apply_model_map       → "claude-opus-4-6" → "glm-5"
   - apply_thinking_compat → adaptive → enabled + budget
   - NO filter (thinking is None)
   - send to glm
   - NO register (thinking is None)
```

## Изменения в файлах

### Изменяется

| Файл | Изменение |
|------|-----------|
| `src/proxy/thinking/mod.rs` | + `ThinkingSession` struct, + `begin_request()`, - `transformer()`, - pub re-exports мёртвого кода |
| `src/proxy/upstream.rs` | - `transformer_registry` field, - thinking inline logic, + extract `ThinkingSession` from extensions |
| `src/proxy/router.rs` | + `transformer_registry` в `RouterEngine`, + `thinking_middleware`, переделка `build_router()`, + `BackendOverride` |
| `src/proxy/routing.rs` | - `routing_middleware`, - `RoutedTo`, - `rewrite_uri()`, - `RoutingRule` trait, - `PathPrefixRule`. Оставить `build_rules()` → удалить целиком |
| `src/proxy/server.rs` | Передавать `transformer_registry` в `RouterEngine`, убрать из `UpstreamClient::new()`, изменить `build_router()` call |
| `src/proxy/mod.rs` | - `pub mod routing` (если модуль удаляется целиком) |

### Удаляется

| Файл | Причина |
|------|---------|
| `src/proxy/thinking/native.rs` | No-op passthrough, dead code |
| `src/proxy/thinking/context.rs` | `TransformContext` + `TransformResult`, используются только `NativeTransformer` |
| `src/proxy/thinking/error.rs` | `TransformError`, используется только `NativeTransformer` |
| `src/proxy/routing.rs` | Полностью заменяется axum `nest()` + `BackendOverride` Extension |

### Не изменяется

| Файл | Причина |
|------|---------|
| `src/proxy/thinking/registry.rs` | Core логика ThinkingRegistry — без изменений, 52 теста |
| `tests/thinking_registry.rs` | 52 теста — без изменений |
| `tests/thinking_request_structure.rs` | 8 тестов — без изменений |
| `src/shim/tmux.rs` | tmux shim — без изменений |
| `src/shim/claude.rs` | claude shim — без изменений |
| `convert_adaptive_thinking()` | Свободная функция в upstream.rs — без изменений |
| `patch_anthropic_beta_header()` | Свободная функция в upstream.rs — без изменений |

### Тесты routing.rs

`tests/routing.rs` содержит тесты для `RoutingRule`, `PathPrefixRule`,
`routing_middleware`. С удалением `routing.rs` эти тесты удаляются.
Новое поведение (axum `nest()`) покрывается E2E тестами — axum nesting
не требует unit-тестов (это тестирование фреймворка).

## Верификация

1. `cargo build` — компилируется без ошибок
2. `cargo test` — все существующие тесты проходят
3. E2E с Agent Teams:
   - Main agent: debug.log содержит `[thinking_filter]` записи
   - Teammate: debug.log **не** содержит `[thinking_filter]` записей
   - Main agent: thinking блоки не теряются при чередовании с teammate
   - Teammate: model_map работает (`claude-opus-4-6` → `glm-5`)
   - Teammate: thinking_compat работает (`adaptive` → `enabled`)
4. E2E без Agent Teams:
   - `[agent_teams]` не в конфиге → нет nest, нет BackendOverride
   - Поведение идентично текущему
5. Thinking session stability:
   - Запустить main + 2 teammates
   - Main agent: session counter НЕ инкрементируется от teammate запросов
   - Main agent: thinking блоки сохраняются между запросами

## Риски

| Риск | Митигация |
|------|-----------|
| axum `nest()` может работать иначе, чем кастомный routing | Протестировать path stripping, query string forwarding |
| `BackendOverride` в extensions может конфликтовать с другими | Уникальный тип, axum extensions type-safe |
| `thinking_middleware` получает backend name из `get_active_backend()`, а `forward()` тоже — может разойтись | Маловероятно (оба вызова в одном request cycle), а `begin_request()` идемпотентен для одного бэкенда |
| Удаление `routing.rs` теряет гибкость `RoutingRule` trait | YAGNI — сейчас один rule (path prefix), axum `nest()` проще |
