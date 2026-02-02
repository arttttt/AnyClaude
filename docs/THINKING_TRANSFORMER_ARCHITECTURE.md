# Thinking Transformer Architecture

## Исследование существующих решений

### Проанализированные проекты

| Проект | Язык | Паттерн | Применимость |
|--------|------|---------|--------------|
| [claude-code-mux](https://github.com/9j/claude-code-mux) | Rust | Provider abstraction + config-driven | Высокая |
| [llm-edge-agent](https://github.com/globalbusinessadvisors/llm-edge-agent) | Rust | Layered middleware (Axum) | Высокая |
| [nexus](https://github.com/grafbase/nexus) | Rust | Configuration composition | Средняя |
| [tower-llm](https://docs.rs/tower-llm) | Rust | Tower Service/Layer + Codec | Высокая |
| [kairos-rs](https://github.com/DanielSarmiento04/kairos-rs) | Rust | Per-route transformation | Средняя |

### Ключевые паттерны из исследования

**1. Tower Service/Layer (tower-llm)**
- Использует `Arc<dyn Transformer>` для динамической загрузки плагинов
- Codec layer для bidirectional message conversion
- Policy-driven control flow через `CompositePolicy`

**2. Axum Body Transformation Pattern**
```rust
// Decompose → Buffer → Transform → Reconstruct
let (parts, body) = req.into_parts();
let bytes = body.collect().await?.to_bytes();
// ... transform bytes ...
let req = Request::from_parts(parts, Body::from(transformed_bytes));
```

**3. Chain of Responsibility (refactoring.guru)**
- Dynamic dispatch с `Box<dyn Handler>`
- `execute()` + `handle()` + `next()` методы
- Runtime chain construction

**4. async_trait для async + trait objects**
```rust
#[async_trait]
pub trait Handler: Send + Sync {
    async fn handle(&self, request: &mut Request) -> Result<()>;
}
// Можно использовать как Box<dyn Handler>
```

### Выводы для нашего проекта

1. **Trait-based Strategy** — оптимальный выбор для нашего случая:
   - Каждый режим имеет разное поведение
   - Summarize требует async (HTTP к внешнему API)
   - Просто тестировать изолированно
   - Легко добавлять новые режимы

2. **async_trait** — необходим для async методов в trait objects

3. **Factory/Registry** — для горячей замены трансформера при изменении конфига

---

## Текущее состояние

```
ThinkingTracker
    ├── mode: ThinkingMode (enum)
    └── transform_request() -> всё в одном методе с match
```

Проблемы:
- Вся логика в одном методе с `match`
- Нельзя добавить async операции (нужно для `summarize`)
- Сложно тестировать отдельные режимы
- Нет общего интерфейса для расширения

## Новая архитектура

### Core Trait

```rust
/// Трансформер для обработки thinking блоков в запросах.
///
/// Каждый режим работы реализует этот trait.
#[async_trait]
pub trait ThinkingTransformer: Send + Sync {
    /// Имя трансформера для логирования
    fn name(&self) -> &'static str;

    /// Трансформировать запрос перед отправкой upstream.
    ///
    /// Вызывается на КАЖДЫЙ запрос к API.
    async fn transform_request(
        &self,
        body: &mut serde_json::Value,
        context: &TransformContext,
    ) -> Result<TransformResult, TransformError>;

    /// Вызывается при переключении backend (опционально).
    ///
    /// Только `summarize` режим использует это для суммаризации.
    async fn on_backend_switch(
        &self,
        _from: &str,
        _to: &str,
        _body: &mut serde_json::Value,
    ) -> Result<(), TransformError> {
        Ok(()) // По умолчанию - ничего не делаем
    }
}
```

### Context и Result

```rust
/// Контекст для трансформации
pub struct TransformContext {
    /// Текущий backend
    pub backend: String,
    /// ID запроса для трейсинга
    pub request_id: String,
}

/// Результат трансформации
#[derive(Debug, Default)]
pub struct TransformResult {
    /// Было ли изменено тело запроса
    pub changed: bool,
    /// Статистика по операциям
    pub stats: TransformStats,
}

#[derive(Debug, Default)]
pub struct TransformStats {
    pub stripped_count: u32,
    pub summarized_count: u32,
    pub passed_through_count: u32,
}

/// Ошибки трансформации
#[derive(Debug, thiserror::Error)]
pub enum TransformError {
    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Summarization failed: {0}")]
    SummarizationError(String),

    #[error("Backend not available: {0}")]
    BackendError(String),
}
```

### Реализации

```
src/proxy/thinking/
├── mod.rs              # Публичный API, TransformerRegistry
├── traits.rs           # ThinkingTransformer trait
├── context.rs          # TransformContext, TransformResult
├── error.rs            # TransformError
├── strip.rs            # StripTransformer
├── summarize.rs        # SummarizeTransformer (будущее)
└── native.rs           # NativeTransformer (будущее)
```

#### StripTransformer

```rust
/// Режим strip: полностью удаляет thinking блоки.
pub struct StripTransformer;

#[async_trait]
impl ThinkingTransformer for StripTransformer {
    fn name(&self) -> &'static str { "strip" }

    async fn transform_request(
        &self,
        body: &mut Value,
        _context: &TransformContext,
    ) -> Result<TransformResult, TransformError> {
        let mut result = TransformResult::default();

        if let Some(messages) = body.get_mut("messages").and_then(|v| v.as_array_mut()) {
            for message in messages {
                if let Some(content) = message.get_mut("content").and_then(|v| v.as_array_mut()) {
                    // Удаляем все thinking блоки
                    let before_len = content.len();
                    content.retain(|item| {
                        item.get("type").and_then(|t| t.as_str()) != Some("thinking")
                    });
                    result.stats.stripped_count += (before_len - content.len()) as u32;
                }
            }
        }

        result.changed = result.stats.stripped_count > 0;

        // Удаляем context_management если были изменения
        if result.changed {
            if let Some(obj) = body.as_object_mut() {
                obj.remove("context_management");
            }
        }

        Ok(result)
    }
}
```

#### SummarizeTransformer (будущее)

**Принцип работы:**

1. При переключении бэкенда показывается UI-диалог с прогрессом
2. Вызывается LLM (настраиваемая модель) для суммаризации истории сессии
3. Результат сохраняется в памяти
4. При первом запросе к новому бэкенду саммари добавляется к сообщению пользователя (prepend)

**Почему prepend к сообщению, а не system prompt:**
- Это не системная информация — контекст предыдущей сессии
- Используется один раз, не раздувает контекст последующих запросов
- Может меняться со временем

```rust
/// Режим summarize: нативная работа + суммаризация при switch.
pub struct SummarizeTransformer {
    /// Последние сообщения для суммаризации (обновляются при каждом запросе)
    last_messages: RwLock<Option<Vec<Value>>>,
    /// Готовое саммари, ожидающее использования в первом запросе
    pending_summary: RwLock<Option<String>>,
    /// Конфигурация суммаризатора
    config: SummarizeConfig,
    /// HTTP клиент для вызова LLM
    client: reqwest::Client,
}

/// Конфигурация суммаризации
#[derive(Debug, Clone, Deserialize)]
pub struct SummarizeConfig {
    /// Модель для суммаризации ("claude-3-haiku", "gpt-4o-mini", или "current")
    pub model: String,
    /// Бэкенд для суммаризации (если model != "current")
    pub backend: Option<String>,
    /// Максимальное количество токенов в саммари
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Промпт для суммаризации
    #[serde(default = "default_summarize_prompt")]
    pub prompt: String,
}

fn default_max_tokens() -> u32 { 500 }
fn default_summarize_prompt() -> String {
    "Summarize this coding session for handoff to another AI assistant. \
     Focus on: current task, files modified, decisions made, next steps.".into()
}

#[async_trait]
impl ThinkingTransformer for SummarizeTransformer {
    fn name(&self) -> &'static str { "summarize" }

    async fn transform_request(
        &self,
        body: &mut Value,
        _context: &TransformContext,
    ) -> Result<TransformResult, TransformError> {
        let mut result = TransformResult::default();

        // 1. Сохраняем messages для будущей суммаризации
        if let Some(messages) = body.get("messages") {
            *self.last_messages.write().await = Some(
                messages.as_array().cloned().unwrap_or_default()
            );
        }

        // 2. Если есть pending_summary — prepend к первому user message
        if let Some(summary) = self.pending_summary.write().await.take() {
            self.prepend_summary_to_user_message(body, &summary);
            result.stats.summarized_count = 1;
            result.changed = true;
        }

        // 3. Strip thinking блоков (они учтены в summary)
        let strip_result = self.strip_thinking_blocks(body);
        result.stats.stripped_count = strip_result.stats.stripped_count;
        result.changed = result.changed || strip_result.changed;

        Ok(result)
    }

    /// Вызывается ИЗ UI при переключении бэкенда (до переключения).
    /// UI показывает диалог с прогрессом.
    async fn on_backend_switch(
        &self,
        from: &str,
        to: &str,
    ) -> Result<(), TransformError> {
        tracing::info!(from = %from, to = %to, "Summarizing session for backend switch");

        // Получаем сохранённые сообщения
        let messages = self.last_messages.read().await.clone()
            .ok_or_else(|| TransformError::SummarizationError(
                "No messages to summarize".into()
            ))?;

        // Вызываем LLM для суммаризации
        let summary = self.call_summarize_llm(&messages).await?;

        // Сохраняем для использования в первом запросе
        *self.pending_summary.write().await = Some(summary);

        Ok(())
    }
}

impl SummarizeTransformer {
    /// Добавляет саммари в начало первого user message
    fn prepend_summary_to_user_message(&self, body: &mut Value, summary: &str) {
        if let Some(messages) = body.get_mut("messages").and_then(|v| v.as_array_mut()) {
            // Находим первое user сообщение
            for message in messages.iter_mut() {
                if message.get("role").and_then(|r| r.as_str()) == Some("user") {
                    // Prepend summary
                    if let Some(content) = message.get_mut("content").and_then(|c| c.as_str()) {
                        let new_content = format!(
                            "[Session context from previous assistant]\n{}\n\n---\n\n{}",
                            summary, content
                        );
                        message["content"] = Value::String(new_content);
                    }
                    break;
                }
            }
        }
    }

    /// Вызов LLM API для суммаризации
    async fn call_summarize_llm(&self, messages: &[Value]) -> Result<String, TransformError> {
        // Формируем запрос к LLM
        let request_body = json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "messages": [
                {
                    "role": "user",
                    "content": format!(
                        "{}\n\n<session>\n{}\n</session>",
                        self.config.prompt,
                        serde_json::to_string_pretty(messages).unwrap_or_default()
                    )
                }
            ]
        });

        // Отправляем запрос (backend URL и auth берутся из config)
        let response = self.client
            .post(&self.get_summarize_endpoint())
            .json(&request_body)
            .send()
            .await
            .map_err(|e| TransformError::SummarizationError(e.to_string()))?;

        // Парсим ответ
        let response_json: Value = response.json().await
            .map_err(|e| TransformError::SummarizationError(e.to_string()))?;

        // Извлекаем текст ответа
        response_json["content"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| TransformError::SummarizationError(
                "Invalid response format".into()
            ))
    }
}
```

### Registry / Factory

```rust
/// Реестр трансформеров с поддержкой горячей замены.
pub struct TransformerRegistry {
    current: Arc<RwLock<Box<dyn ThinkingTransformer>>>,
}

impl TransformerRegistry {
    pub fn new(config: &ThinkingConfig) -> Self {
        let transformer = Self::create_transformer(config);
        Self {
            current: Arc::new(RwLock::new(transformer)),
        }
    }

    fn create_transformer(config: &ThinkingConfig) -> Box<dyn ThinkingTransformer> {
        match config.mode {
            ThinkingMode::Strip => Box::new(StripTransformer),
            ThinkingMode::Summarize => Box::new(SummarizeTransformer::new(&config.summarizer)),
            ThinkingMode::Native => Box::new(NativeTransformer),
        }
    }

    /// Обновить конфигурацию (горячая замена)
    pub fn update_config(&self, config: &ThinkingConfig) {
        let transformer = Self::create_transformer(config);
        *self.current.write() = transformer;
    }

    pub fn get(&self) -> Arc<RwLock<Box<dyn ThinkingTransformer>>> {
        self.current.clone()
    }
}
```

## Интеграция с UpstreamClient

```rust
// upstream.rs

impl UpstreamClient {
    pub async fn do_forward(...) -> Result<Response<Body>, ProxyError> {
        // ...

        if request_content_type.contains("application/json") {
            let context = TransformContext {
                backend: backend.name.clone(),
                request_id: span.request_id().to_string(),
            };

            // Получаем текущий трансформер
            let transformer = self.transformer_registry.get();
            let transformer = transformer.read();

            // Трансформируем (async!)
            let result = transformer
                .transform_request(&mut json_body, &context)
                .await
                .map_err(|e| ProxyError::Internal(e.to_string()))?;

            if result.changed {
                body_bytes = serde_json::to_vec(&json_body)?;
                tracing::info!(
                    transformer = transformer.name(),
                    stats = ?result.stats,
                    "Transformed thinking blocks"
                );
            }
        }

        // ...
    }
}
```

## Интеграция с UI (событие переключения бэкенда)

При переключении бэкенда (Summarize mode) нужен UI-диалог с прогрессом:

```
┌─────────────────────────────────────────────────────────────┐
│          Switching to GLM-4                                 │
│                                                             │
│     [████████████░░░░░░░░] Summarizing session...          │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Поток событий

```
User clicks "Switch to GLM"
         ↓
┌────────────────────────────────────────────────────────────┐
│ IPC Handler (backend_switch command)                       │
├────────────────────────────────────────────────────────────┤
│ 1. НЕ переключаем бэкенд сразу                             │
│ 2. Проверяем режим: if mode == Summarize                   │
│ 3. Отправляем UI событие "show_summarize_progress"         │
│ 4. Вызываем transformer.on_backend_switch(from, to).await  │
│ 5. Отправляем UI событие "hide_summarize_progress"         │
│ 6. ТЕПЕРЬ переключаем бэкенд                               │
└────────────────────────────────────────────────────────────┘
```

### Код интеграции

```rust
// ipc/handler.rs

async fn handle_switch_backend(
    &self,
    target_backend: String,
) -> Result<IpcResponse, IpcError> {
    let current_backend = self.backend_state.get_active_backend();

    // Если режим Summarize — нужна суммаризация перед переключением
    if self.config.get().thinking.mode == ThinkingMode::Summarize {
        // Уведомляем UI о начале суммаризации
        self.ui_sender.send(UiEvent::ShowSummarizeProgress {
            from: current_backend.clone(),
            to: target_backend.clone(),
        })?;

        // Вызываем on_backend_switch (async LLM call)
        let transformer = self.transformer_registry.get().await;
        if let Err(e) = transformer.on_backend_switch(&current_backend, &target_backend).await {
            tracing::error!(error = %e, "Failed to summarize session");
            // Продолжаем переключение даже при ошибке суммаризации
        }

        // Уведомляем UI о завершении
        self.ui_sender.send(UiEvent::HideSummarizeProgress)?;
    }

    // Теперь переключаем бэкенд
    self.backend_state.switch_backend(&target_backend)?;

    Ok(IpcResponse::BackendSwitched { backend: target_backend })
}
```

### UI события

```rust
pub enum UiEvent {
    // ... existing events ...

    /// Показать диалог прогресса суммаризации
    ShowSummarizeProgress {
        from: String,
        to: String,
    },

    /// Скрыть диалог прогресса
    HideSummarizeProgress,

    /// Ошибка суммаризации (опционально показать)
    SummarizeError {
        error: String,
    },
}
```

## Миграция

### Phase 0: Инфраструктура ✅ DONE

```bash
src/proxy/thinking/
├── mod.rs         # TransformerRegistry
├── traits.rs      # ThinkingTransformer trait
├── context.rs     # TransformContext, TransformResult
├── error.rs       # TransformError
└── strip.rs       # StripTransformer
```

- [x] Создать модульную структуру
- [x] Реализовать ThinkingTransformer trait с async_trait
- [x] Реализовать TransformerRegistry с tokio::sync::RwLock
- [x] Интегрировать с UpstreamClient
- [x] Удалить старый ThinkingTracker

### Phase 1: Strip Mode ✅ DONE

- [x] Реализовать StripTransformer
- [x] Тесты для strip режима
- [x] Удалить legacy режимы (DropSignature, ConvertToText, ConvertToTags)

### Phase 2: Summarize Mode 🔄 IN PROGRESS

#### Phase 2.1: Конфигурация ✅ DONE
- [x] 2.1.1: Добавить `SummarizeConfig` структуру в `src/config/types.rs`
- [x] 2.1.2: Добавить `summarize: SummarizeConfig` в `ThinkingConfig`
- [x] 2.1.3: Дефолтные значения и serde аннотации
- [x] 2.1.4: Тест парсинга TOML с секцией `[thinking.summarize]`

#### Phase 2.2: SummarizeTransformer каркас ✅ DONE
- [x] 2.2.1: Создать файл `src/proxy/thinking/summarize.rs`
- [x] 2.2.2: Структура с полями `last_messages`, `pending_summary`, `config`
- [x] 2.2.3: Конструктор `new(config: SummarizeConfig)`
- [x] 2.2.4: Реализация `name()` → "summarize"
- [x] 2.2.5: Базовый `transform_request` — сохранение messages + strip thinking
- [x] 2.2.6: Добавить в `mod.rs` и `TransformerRegistry::create_transformer`
- [x] 2.2.7: Обновить `router.rs` — передавать `ThinkingConfig` вместо `ThinkingMode`
- [x] 2.2.8: Тесты: registry_creates_summarize_transformer, registry_with_full_config

#### Phase 2.3: Strip логика в Summarize ✅ DONE
- [x] 2.3.1: Вынести strip логику в `strip.rs` как `pub fn strip_thinking_blocks()`
- [x] 2.3.2: Добавить `pub fn remove_context_management()` в `strip.rs`
- [x] 2.3.3: `SummarizeTransformer` импортирует и использует функции из `strip.rs`
- [x] 2.3.4: Существующие тесты покрывают strip в контексте Summarize

#### Phase 2.4: Prepend логика ✅ DONE
- [x] 2.4.1: Функция `prepend_summary_to_user_message(body, summary)` — обрабатывает string и array content
- [x] 2.4.2: Интеграция в `transform_request` — берёт `pending_summary`, prepend, очищает
- [x] 2.4.3: Тесты: string content, array с text, array без text, no user message, integration

#### Phase 2.5: LLM клиент ✅ DONE
- [x] 2.5.1: Создать `SummarizerClient` в `src/proxy/thinking/summarizer.rs`
- [x] 2.5.2: Обновить `SummarizeConfig`: убрать `prompt`/`backend`, добавить `base_url`/`api_key`
- [x] 2.5.3: Default endpoint: `https://api.z.ai/api/anthropic` (Anthropic-compatible)
- [x] 2.5.4: Hardcoded промпт в коде (MVP approach)
- [x] 2.5.5: `SummarizeError` enum: NotConfigured, Network, ApiError, ParseError, EmptyResponse
- [x] 2.5.6: Unit тесты + integration тест (requires SUMMARIZER_API_KEY)
- [x] 2.5.7: Env var: `SUMMARIZER_API_KEY` (generic name, not vendor-specific)

#### Phase 2.6: MVI architecture + Summarization UI 🔄 IN PROGRESS
- [x] 2.6.1: MVI infrastructure в `src/ui/mvi/`:
  - `state.rs` — trait UiState
  - `intent.rs` — trait Intent
  - `reducer.rs` — trait Reducer
- [x] 2.6.2: Summarization feature в `src/ui/summarization/`:
  - `state.rs` — SummarizeDialogState (Hidden, Summarizing, Retrying, Failed, Success)
  - `intent.rs` — SummarizeIntent (Start, AnimationTick, Error, Success, Retry/Cancel)
  - `reducer.rs` — SummarizeReducer (state machine with 3 auto-retries)
  - `dialog.rs` — render_summarize_dialog (spinner animation, inline buttons)
- [ ] 2.6.3: Валидация на старте: crash если mode=summarize и нет API key
- [ ] 2.6.4: Интеграция `SummarizerClient` в `SummarizeTransformer`
- [ ] 2.6.5: Реализация `on_backend_switch` с retry логикой

#### Phase 2.7: IPC + UI интеграция
- [ ] 2.7.1: IPC события: SummarizationStarted, Progress, Error, Completed
- [ ] 2.7.2: Интеграция диалога поверх BackendSwitch popup
- [ ] 2.7.3: Input handling для Retry/Cancel кнопок
- [ ] 2.7.4: Animation tick timer для спиннера

#### Phase 2.8: Polish
- [ ] 2.8.1: Error recovery flow (user choice after max retries)
- [ ] 2.8.2: Cancel подтверждение
- [ ] 2.8.3: Success auto-close с delay

### Phase 3: Native Mode 📋 FUTURE

- [ ] Дизайн handoff механизма
- [ ] NativeTransformer реализация
- [ ] UI для handoff диалога

## Конфигурация

```toml
[thinking]
mode = "strip"  # "strip" | "summarize" | "native"

# Настройки для summarize режима
[thinking.summarize]
# Base URL для Anthropic-compatible API
base_url = "https://api.z.ai/api/anthropic"

# API ключ (опционально, можно через env var SUMMARIZER_API_KEY)
api_key = "your-api-key"

# Модель для суммаризации
model = "glm-4.7"

# Максимальное количество токенов в саммари
max_tokens = 500
```

### SummarizeConfig structure

```rust
pub struct SummarizeConfig {
    /// Base URL for Anthropic-compatible API
    pub base_url: String,      // default: "https://api.z.ai/api/anthropic"

    /// API key (or use SUMMARIZER_API_KEY env var)
    pub api_key: Option<String>,

    /// Model name
    pub model: String,          // default: "glm-4.7"

    /// Max tokens in summary
    pub max_tokens: u32,        // default: 500
}
```

Note: Prompt is hardcoded in code (MVP approach) for simplicity.

### Примеры конфигураций

**Z.ai GLM (default):**
```toml
[thinking]
mode = "summarize"

[thinking.summarize]
# Uses defaults: base_url = "https://api.z.ai/api/anthropic", model = "glm-4.7"
# API key from SUMMARIZER_API_KEY env var
max_tokens = 300
```

**Custom Anthropic-compatible endpoint:**
```toml
[thinking]
mode = "summarize"

[thinking.summarize]
base_url = "https://your-endpoint.com/v1"
api_key = "your-key"
model = "custom-model"
max_tokens = 500
```

## Тестирование

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn strip_transformer_removes_thinking_blocks() {
        let transformer = StripTransformer;
        let mut body = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "my thoughts", "signature": "sig"},
                    {"type": "text", "text": "hello"}
                ]
            }]
        });

        let context = TransformContext {
            backend: "test".to_string(),
            request_id: "test-123".to_string(),
        };

        let result = transformer.transform_request(&mut body, &context).await.unwrap();

        assert!(result.changed);
        assert_eq!(result.stats.stripped_count, 1);

        // Проверяем что thinking блок удалён
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }
}
```

## Преимущества новой архитектуры

1. **Расширяемость**: Новый режим = новый файл с реализацией trait
2. **Тестируемость**: Каждый трансформер тестируется изолированно
3. **Async-ready**: `summarize` может делать HTTP запросы
4. **Горячая замена**: Можно менять режим без перезапуска
5. **Чистый код**: Разделение ответственности
6. **Типобезопасность**: Ошибки ловятся на этапе компиляции

## Зависимости

```toml
# Cargo.toml additions
async-trait = "0.1"      # Для async методов в trait objects
thiserror = "1.0"        # Для TransformError (уже используется)
```

## Альтернативы рассмотренные и отклонённые

### 1. Enum + match (текущий подход)
**Отклонён**: Вся логика в одном методе, не масштабируется, нет async.

### 2. Tower Layer
**Отклонён**: Overkill для нашего случая. Tower хорош для цепочек middleware, но мы выбираем ОДИН трансформер на основе конфига.

### 3. Dynamic plugin loading (FFI)
**Отклонён**: Требует отдельные бинарники, unstable ABI, избыточная сложность.

### 4. WASM plugins
**Отклонён**: Overhead, ограничения песочницы, сложность разработки.
