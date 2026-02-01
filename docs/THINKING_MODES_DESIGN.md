# Thinking Modes Design

## Проблема

При использовании ClaudeWrapper в режиме `convert_to_tags` thinking блоки конвертируются в обычный текст с тегами `<think>...</think>`. Это создаёт проблемы:

1. **Накопление контекста**: API Anthropic не распознаёт `<think>` как thinking блоки и не применяет автоматическое stripping/суммаризацию
2. **Потеря суммаризации**: Anthropic использует отдельную модель для суммаризации thinking блоков — при конвертации в текст эта функция теряется
3. **Раздувание контекста**: Сырые thinking блоки накапливаются, занимая место в контексте
4. **Зависание extended thinking**: В некоторых случаях это приводит к зависанию стрима
5. **Потеря signature**: При конвертации теряется криптографическая подпись (см. ниже)

---

## Анатомия Thinking блока (важно!)

### Структура блока

```json
{
  "type": "thinking",
  "thinking": "Let me analyze this problem step by step...",
  "signature": "WyJhbnRocm9waWMiLCIxMjM0NTY3ODkw..."
}
```

**Signature** — криптографическая подпись, которая:
- Генерируется API Anthropic для каждого thinking блока
- **Валидируется** при последующих запросах
- **НЕЛЬЗЯ** модифицировать, урезать или генерировать
- При несовпадении — API возвращает 400 error

### Требования API (из официального cookbook)

```
┌─────────────────────────────────────────────────────────────┐
│  Порядок блоков в assistant message (СТРОГО):               │
│                                                              │
│  1. [thinking/redacted_thinking] ← ОБЯЗАТЕЛЬНО первый       │
│  2. [tool_use]                   ← После thinking           │
│  3. ...                                                      │
└─────────────────────────────────────────────────────────────┘
```

**Правила:**
1. При `thinking: enabled` — assistant message **ДОЛЖЕН** начинаться с thinking блока
2. Thinking блок **ДОЛЖЕН** содержать оригинальный signature
3. После `tool_result` Claude **НЕ** генерирует новый thinking (только после user message)
4. Блоки передаются **as-is** — любая модификация invalidates signature

### Redacted Thinking

```json
{
  "type": "redacted_thinking",
  "data": "eyJhbGciOiJIUzI1NiIs..."  // Зашифрованные данные
}
```

Иногда Claude возвращает `redacted_thinking` вместо `thinking`:
- Содержимое зашифровано (поле `data`)
- **Также требует сохранения** в истории
- Signature не нужен (данные уже подписаны)

### Влияние на наши режимы

| Режим | Signature | Tool Use |
|-------|-----------|----------|
| `strip` | Не важен (удаляем блок) | ✅ Работает |
| `summarize` | Сохраняем до switch, затем теряем | ⚠️ После switch tools могут сломаться |
| `native` | Полностью сохраняется | ✅ Работает |

### Известные проблемы в других проектах

| Проект | Проблема | Причина |
|--------|----------|---------|
| LiteLLM | 400 при tool use | Не сохраняют signature в streaming |
| OpenCode | "must start with thinking" | Нарушен порядок блоков |
| Cline | "Invalid signature" | Смена моделей без очистки |
| Vercel AI | Потеря reasoning | Не persist-ят полные блоки |

## Решение: Три режима работы с thinking

### Обзор режимов

| Режим | Backend switching | Thinking контекст | Перезапуск Claude | Сложность |
|-------|------------------|-------------------|-------------------|-----------|
| `strip` | Мгновенно | Теряется | Нет | Низкая |
| `summarize` | С суммаризацией | Сохраняется (сжато) | **Нет** | Средняя |
| `native` | Через handoff | Полный (нативный) | **Да** | Высокая |

### Когда какой режим использовать

- **`strip`** — когда thinking не важен, или нужна максимальная стабильность
- **`summarize`** — **рекомендуется** для большинства случаев; баланс между сохранением контекста и гибкостью
- **`native`** — для сложных задач, где важен полный thinking контекст и редко нужно переключать backend

---

## Режим 1: `strip`

### Описание

Полное удаление thinking блоков из запросов. Самый простой режим, который гарантирует стабильную работу при переключении backends.

### Поведение

1. При получении запроса с thinking блоками в истории
2. Thinking блоки **полностью удаляются** (не конвертируются в текст)
3. Запрос отправляется без thinking контекста
4. Переключение backends работает без ограничений

### Конфигурация

```toml
[thinking]
mode = "strip"
```

### Преимущества

- Простая реализация
- Нет накопления контекста
- Стабильная работа с любыми backends
- Нет зависимостей

### Недостатки

- Потеря контекста размышлений между ходами
- Модель "забывает" свои предыдущие рассуждения

### Реализация

```rust
// В thinking.rs
ThinkingMode::Strip => {
    // Полностью удаляем thinking блок из массива content
    items_to_remove.push(index);
    result.strip_count = result.strip_count.saturating_add(1);
    changed = true;
}
```

### Открытые вопросы

- [ ] Нужно ли логировать количество удалённых блоков?
- [ ] Показывать ли предупреждение пользователю при первом удалении?

---

## Режим 2: `summarize`

### Описание

Гибридный режим: во время работы с одним backend thinking блоки **сохраняются в нативном формате** (как в режиме `native`). Суммаризация происходит **только в момент переключения backend** — все накопленные thinking блоки суммаризируются через отдельную модель и заменяются на текстовое резюме.

### Ключевое отличие от других режимов

| Аспект | `strip` | `summarize` | `native` |
|--------|---------|-------------|----------|
| Во время работы | Удаление | **Нативный формат** | Нативный формат |
| При переключении | Просто удалить | **Суммаризировать** | Handoff + restart |
| Перезапуск Claude | Нет | **Нет** | Да |

### Поведение

**Обычная работа (без переключения backend):**
1. Thinking блоки передаются **как есть** (нативный формат Anthropic)
2. API Anthropic сам управляет thinking (автосуммаризация, stripping)
3. Полноценная работа extended thinking
4. Нет дополнительных API вызовов

**При переключении backend:**
1. Пользователь запрашивает смену backend (например, Anthropic → GLM)
2. ClaudeWrapper показывает UI: "Preparing switch..."
3. Для каждого assistant message в истории:
   - Thinking блоки → суммаризируются через summarizer модель
   - Tool_use + tool_result блоки → суммаризируются в список действий
   - Оба summary объединяются в текстовый блок
4. Переключение завершается
5. **Claude продолжает работу без перезапуска** — новые запросы идут к новому backend

**Почему удаляем tool_use/tool_result:**
- Signature в thinking блоках привязан к backend
- API требует thinking блок перед tool_use (при включенном thinking)
- После switch signature невалиден → API отклонит запрос
- Решение: суммаризируем и thinking, и tools в текст

### Конфигурация

```toml
[thinking]
mode = "summarize"

[thinking.summarizer]
# Какой backend использовать для суммаризации
backend = "glm"  # Имя из секции [[backends]]

# Опционально: конкретная модель (если backend поддерживает несколько)
model = "glm-4-flash"

# Максимальное количество токенов в резюме
max_tokens = 500

# Формат вывода резюме
output_format = "text"  # "text" | "xml" | "json"

# Кэширование резюме (по хэшу содержимого)
cache_enabled = true
cache_ttl_seconds = 3600
```

### Промпт для суммаризации

```
[SYSTEM]
You are a summarization assistant. Summarize the following AI turn including
both reasoning and actions taken.

Output in this exact format:
<reasoning>
Brief summary of the reasoning process and key decisions
</reasoning>
<actions>
- List of actions taken with key parameters
</actions>

Be concise. Preserve critical context. No preamble.

[USER]
THINKING:
{thinking_content}

ACTIONS:
{tool_calls_with_arguments_and_results}
```

### Формат вывода

**Рекомендуемый: XML tags (два раздела)**

До суммаризации:
```json
{
  "role": "assistant",
  "content": [
    {"type": "thinking", "thinking": "Need to check config...", "signature": "abc"},
    {"type": "tool_use", "name": "Read", "input": {"path": "config.json"}},
  ]
}
// + tool_result от user
// + финальный text response
```

После суммаризации:
```json
{
  "role": "assistant",
  "content": [
    {
      "type": "text",
      "text": "<reasoning>\nAnalyzed project structure, decided to check configuration for port settings.\n</reasoning>\n<actions>\n- Read config.json to check port configuration\n- Found port 8080 is hardcoded\n</actions>"
    },
    {"type": "text", "text": "Конфиг содержит порт 8080..."}
  ]
}
```

**Преимущества XML формата:**
- Структурированный, легко парсить при необходимости
- Модель понимает разделение reasoning/actions
- Не конфликтует с другими форматами в контексте

### Flow переключения backend

```
┌─────────────────────────────────────────────────────────────────┐
│                  ClaudeWrapper (summarize mode)                  │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  [Обычная работа: thinking нативный, API сам его обрабатывает]  │
│                                                                  │
│  User: "Switch to GLM"                                          │
│           │                                                      │
│           ▼                                                      │
│  ┌─────────────────────────────────────────┐                    │
│  │ UI: "Preparing backend switch..."       │                    │
│  │      [████████░░░░░░] Summarizing...    │                    │
│  └─────────────────────────────────────────┘                    │
│           │                                                      │
│           ▼                                                      │
│  ┌─────────────────────────────────────────┐                    │
│  │ Для каждого assistant message:          │                    │
│  │  1. Собрать thinking блоки              │                    │
│  │  2. Собрать tool_use + tool_result      │                    │
│  │  3. Отправить к summarizer              │                    │
│  │  4. Получить <reasoning> + <actions>    │                    │
│  │  5. Заменить на текстовый блок          │                    │
│  └─────────────────────────────────────────┘                    │
│           │                                                      │
│           ▼                                                      │
│  ┌─────────────────────────────────────────┐                    │
│  │ История теперь содержит:                │                    │
│  │  - [text: <reasoning>...</reasoning>    │                    │
│  │         <actions>...</actions>]         │                    │
│  │  - [text: оригинальные ответы]          │                    │
│  └─────────────────────────────────────────┘                    │
│           │                                                      │
│           ▼                                                      │
│  ┌─────────────────────────────────────────┐                    │
│  │ Переключить active backend → GLM        │                    │
│  │ Claude продолжает без перезапуска       │                    │
│  └─────────────────────────────────────────┘                    │
│           │                                                      │
│           ▼                                                      │
│  Пользователь продолжает работу с GLM backend                   │
│  (thinking резюме сохранены в истории как text)                 │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Преимущества

- **Нативная работа thinking** во время обычной сессии
- API сам оптимизирует thinking (суммаризация, кэширование)
- Thinking блоки не считаются в контекст (API их strip-ит)
- Сохраняет контекст **и размышлений, и действий** при переключении
- **Не требует перезапуска Claude** (в отличие от `native`)
- Суммаризация происходит **только при переключении**, не на каждый запрос
- **Нет проблем с signature** — tool_use блоки тоже заменяются на текст

### Недостатки

- Добавляет latency **при переключении** (запрос к summarizer)
- Требует настройки summarizer backend
- Потеря деталей при суммаризации (только при switch)
- После переключения модель не знает точные параметры прошлых tool calls
- Потенциально модель может повторить уже выполненные действия

### ✅ Решение проблемы Signature и Tool Use

**Проблема:** После суммаризации signature теряется. API требует thinking блок с signature перед tool_use.

**Решение:** Суммаризируем **и thinking, и tool_use/tool_result** в единый текстовый блок:

```
До:
[thinking+signature] → [tool_use] → | [tool_result] | → [text]

После:
[text: <reasoning>...</reasoning><actions>...</actions>] → [text]
```

**Почему это работает:**
- Нет tool_use блоков → API не требует thinking перед ними
- Контекст действий сохранён в `<actions>` секции
- Модель понимает что делала раньше из текстового описания

### Реализация

```rust
/// Данные для суммаризации одного assistant turn
struct TurnData {
    thinking_blocks: Vec<String>,
    tool_calls: Vec<ToolCallInfo>,
}

struct ToolCallInfo {
    name: String,
    input: serde_json::Value,
    result: Option<String>,
}

pub struct HistorySummarizer {
    client: Client,
    config: SummarizerConfig,
}

impl HistorySummarizer {
    /// Суммаризировать историю при переключении backend
    pub async fn summarize_for_switch(
        &self,
        messages: &mut Vec<Message>
    ) -> Result<SummarizeResult> {
        let mut stats = SummarizeResult::default();

        // Группируем сообщения по turn (assistant + следующий user с tool_result)
        let turns = self.extract_turns(messages);

        for turn in turns {
            // 1. Собираем thinking и tools
            let turn_data = self.collect_turn_data(&turn);

            if turn_data.is_empty() {
                continue;
            }

            // 2. Суммаризируем через внешнюю модель
            let summary = self.call_summarizer(&turn_data).await?;

            // 3. Заменяем блоки на текстовое резюме
            self.replace_with_summary(&mut turn, summary)?;

            stats.thinking_summarized += turn_data.thinking_blocks.len();
            stats.tools_summarized += turn_data.tool_calls.len();
        }

        Ok(stats)
    }

    fn build_prompt(&self, data: &TurnData) -> String {
        let mut prompt = String::new();

        if !data.thinking_blocks.is_empty() {
            prompt.push_str("THINKING:\n");
            for t in &data.thinking_blocks {
                prompt.push_str(t);
                prompt.push_str("\n\n");
            }
        }

        if !data.tool_calls.is_empty() {
            prompt.push_str("ACTIONS:\n");
            for tool in &data.tool_calls {
                prompt.push_str(&format!("- {}({})", tool.name, tool.input));
                if let Some(result) = &tool.result {
                    let truncated = truncate(result, 200);
                    prompt.push_str(&format!(" → {}", truncated));
                }
                prompt.push('\n');
            }
        }

        prompt
    }
}

// В логике переключения backend
pub async fn switch_backend(&mut self, new_backend: &str) -> Result<()> {
    if self.thinking_mode == ThinkingMode::Summarize {
        self.show_progress("Preparing backend switch...");

        let result = self.summarizer
            .summarize_for_switch(&mut self.message_history)
            .await?;

        tracing::info!(
            thinking = result.thinking_summarized,
            tools = result.tools_summarized,
            "Summarized history for backend switch"
        );
    }

    self.active_backend = new_backend.to_string();
    Ok(())
}
```

### Открытые вопросы

- [ ] Как обрабатывать ошибки summarizer? Fallback на strip?
- [ ] Суммаризировать каждый turn отдельно или batch все turns?
- [ ] Ограничивать ли размер tool_result перед отправкой summarizer?
- [ ] Как показывать прогресс суммаризации в TUI?
- [ ] Кэшировать ли результаты суммаризации?
- [ ] Кэшировать ли резюме на диск (для повторных switch)?
- [ ] Что делать при частом переключении туда-обратно?

---

## Режим 3: `native`

### Описание

Thinking блоки сохраняются в нативном формате API Anthropic. Это позволяет API самостоятельно управлять thinking (суммаризация, stripping). При переключении backend **требуется полный перезапуск Claude** с handoff.

### Отличие от `summarize`

| Аспект | `summarize` | `native` |
|--------|-------------|----------|
| Суммаризация | Внешняя модель (при switch) | Сам Claude (handoff) |
| Перезапуск | **Нет** | **Да** |
| История | Сохраняется | Только summary |
| Сложность | Средняя | Высокая |

> **Когда выбрать `native` вместо `summarize`:** Когда критически важно, чтобы handoff summary делал сам Claude (лучше понимает контекст), и допустима потеря истории сообщений.

### Поведение

**Обычная работа:**
1. Thinking блоки передаются как есть (без трансформации)
2. API Anthropic сам управляет thinking (суммаризация меньшей моделью)
3. Переключение backends **заблокировано**

**При попытке переключения backend:**
1. ClaudeWrapper показывает предупреждение
2. Пользователь подтверждает переключение
3. Текущая сессия Claude получает запрос на создание handoff summary
4. Summary сохраняется в память ClaudeWrapper
5. Текущий процесс Claude завершается
6. Новый процесс Claude запускается с новым backend
7. Handoff summary инжектится в новую сессию

### Конфигурация

```toml
[thinking]
mode = "native"

[thinking.native]
# Показывать предупреждение при попытке переключения
warn_on_switch = true

# Автоматически создавать handoff (без подтверждения)
auto_handoff = false

# Промпт для создания handoff summary
handoff_prompt = """
Please create a concise summary of our conversation for handoff to a new session.
Include:
- What we were working on
- Key decisions made
- Current state of the work
- What remains to be done
"""

# Максимальная длина handoff summary
max_handoff_tokens = 2000

# Формат инжекта handoff в новую сессию
handoff_inject_method = "first_message"  # "first_message" | "system_prompt" | "claude_md"
```

### Flow переключения backend

```
┌─────────────────────────────────────────────────────────────────┐
│                    ClaudeWrapper (native mode)                   │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  User: "Switch to GLM"                                          │
│           │                                                      │
│           ▼                                                      │
│  ┌─────────────────────────────────────────┐                    │
│  │ Warning: Native mode requires handoff.  │                    │
│  │ Current thinking context will be        │                    │
│  │ summarized. Continue? [Y/n]             │                    │
│  └─────────────────────────────────────────┘                    │
│           │                                                      │
│           ▼ (User confirms)                                      │
│  ┌─────────────────────────────────────────┐                    │
│  │ Request handoff summary from            │                    │
│  │ current Claude session (anthropic)      │                    │
│  └─────────────────────────────────────────┘                    │
│           │                                                      │
│           ▼                                                      │
│  ┌─────────────────────────────────────────┐                    │
│  │ Claude (anthropic) returns summary:     │                    │
│  │ "We were implementing thinking modes..."│                    │
│  └─────────────────────────────────────────┘                    │
│           │                                                      │
│           ▼                                                      │
│  ┌─────────────────────────────────────────┐                    │
│  │ Store summary in memory                 │                    │
│  │ Kill Claude process (anthropic)         │                    │
│  │ Start Claude process (glm)              │                    │
│  └─────────────────────────────────────────┘                    │
│           │                                                      │
│           ▼                                                      │
│  ┌─────────────────────────────────────────┐                    │
│  │ Inject handoff into new session:        │                    │
│  │ "[HANDOFF] {summary} [/HANDOFF]"        │                    │
│  └─────────────────────────────────────────┘                    │
│           │                                                      │
│           ▼                                                      │
│  New session continues with GLM backend                         │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Преимущества

- Полноценная работа extended thinking
- API сам оптимизирует thinking (суммаризация, кэширование)
- Thinking блоки не считаются в контекст (API их strip-ит)
- Лучшее качество рассуждений

### Недостатки

- Нельзя переключать backends без перезапуска
- Сложная реализация handoff
- Потеря истории сообщений при handoff (только summary)
- Требуется UI для предупреждений и подтверждений

### Реализация

```rust
// В backend switching logic
pub async fn switch_backend(&mut self, new_backend: &str) -> Result<()> {
    if self.thinking_mode == ThinkingMode::Native {
        // 1. Показать предупреждение и получить подтверждение
        if !self.confirm_native_switch().await? {
            return Ok(()); // User cancelled
        }

        // 2. Запросить handoff summary
        let summary = self.request_handoff_summary().await?;

        // 3. Сохранить summary
        self.pending_handoff = Some(HandoffData {
            summary,
            source_backend: self.active_backend.clone(),
            target_backend: new_backend.to_string(),
            timestamp: Utc::now(),
        });

        // 4. Завершить текущий процесс
        self.pty.terminate().await?;

        // 5. Сменить backend
        self.active_backend = new_backend.to_string();

        // 6. Запустить новый процесс
        self.spawn_claude_process().await?;

        // 7. Инжектить handoff
        self.inject_handoff().await?;

        Ok(())
    } else {
        // Для других режимов — обычное переключение
        self.active_backend = new_backend.to_string();
        Ok(())
    }
}
```

### Открытые вопросы

- [ ] Как показывать предупреждение в TUI? Popup? Отдельный prompt?
- [ ] Как обрабатывать ситуацию, когда Claude не может создать summary?
- [ ] Сохранять ли handoff на диск (для recovery)?
- [ ] Как инжектить handoff? stdin, system prompt, CLAUDE.md?
- [ ] Нужно ли сохранять историю handoff-ов?
- [ ] Как обрабатывать handoff при аварийном завершении?

---

## Конфигурация

### Полная схема конфига

```toml
[thinking]
# Режим работы с thinking блоками
# - "strip": удалять thinking блоки (default)
# - "summarize": суммаризировать через отдельную модель
# - "native": сохранять нативный формат, handoff при смене backend
mode = "strip"

# ============================================
# Настройки для режима "summarize"
# ============================================
[thinking.summarizer]
# Backend для суммаризации (из [[backends]])
backend = "glm"

# Конкретная модель (опционально, если backend поддерживает)
model = "glm-4-flash"

# Максимум токенов в резюме
max_tokens = 500

# Формат вывода: "text", "xml", "json"
output_format = "text"

# Кастомный промпт (опционально)
# prompt = "Summarize the following thinking..."

# Кэширование
cache_enabled = true
cache_ttl_seconds = 3600

# Fallback при ошибке summarizer
fallback_mode = "strip"  # "strip" | "error"

# ============================================
# Настройки для режима "native"
# ============================================
[thinking.native]
# Предупреждение при переключении backend
warn_on_switch = true

# Автоматический handoff без подтверждения
auto_handoff = false

# Промпт для handoff summary
handoff_prompt = """
Create a concise summary for session handoff:
- What we were working on
- Key decisions made
- Current state
- Remaining tasks
"""

# Максимум токенов в handoff
max_handoff_tokens = 2000

# Метод инжекта: "first_message", "system_prompt", "claude_md"
handoff_inject_method = "first_message"

# Сохранять handoff на диск
persist_handoff = false
handoff_path = "~/.cache/claudewrapper/handoffs/"
```

### Миграция с текущего конфига

Текущий конфиг:
```toml
[thinking]
mode = "convert_to_tags"
```

Новый конфиг (обратная совместимость):
```toml
[thinking]
# "convert_to_tags" → deprecated, трактуется как "strip" с предупреждением
mode = "strip"
```

---

## План реализации

### Фаза 1: `strip` режим
**Приоритет: Высокий**
**Сложность: Низкая**

- [ ] Добавить `Strip` вариант в `ThinkingMode` enum
- [ ] Реализовать удаление thinking блоков в `transform_request`
- [ ] Обновить конфиг парсер
- [ ] Добавить тесты
- [ ] Обновить документацию
- [ ] Deprecate `convert_to_tags` режим

### Фаза 2: `summarize` режим (рекомендуемый)
**Приоритет: Высокий**
**Сложность: Средняя**

Ключевое отличие от первоначального дизайна: суммаризация происходит **только при переключении backend**, а не на каждый запрос.

- [ ] Реализовать passthrough thinking блоков при обычной работе
- [ ] Спроектировать интерфейс `ThinkingSummarizer`
- [ ] Реализовать HTTP клиент для summarizer
- [ ] Добавить конфигурацию `[thinking.summarizer]`
- [ ] Интегрировать в логику `switch_backend`
- [ ] Добавить UI прогресса "Preparing switch..."
- [ ] Реализовать кэширование (опционально)
- [ ] Добавить fallback на `strip` при ошибках
- [ ] Добавить тесты
- [ ] Написать документацию

### Фаза 3: `native` режим
**Приоритет: Низкий**
**Сложность: Высокая**

Ключевое отличие от `summarize`: требует **перезапуска Claude** при переключении backend. Это более сложный сценарий, но сохраняет полный thinking контекст в нативном формате.

- [ ] Реализовать блокировку переключения backend
- [ ] Добавить UI для предупреждений (TUI popup)
- [ ] Реализовать запрос handoff summary от текущей сессии Claude
- [ ] Реализовать сохранение handoff в память
- [ ] Реализовать **перезапуск Claude процесса**
- [ ] Реализовать инжект handoff в новую сессию
- [ ] Добавить конфигурацию `[thinking.native]`
- [ ] Добавить persistence (опционально)
- [ ] Добавить тесты
- [ ] Написать документацию

> **Примечание:** Рассмотреть реализацию только после того, как `summarize` режим покажет недостаточное качество сохранения контекста.

---

## Appendix

### A. Как Anthropic обрабатывает thinking

Из документации Anthropic:

> "Thinking blocks from previous turns are stripped and not counted towards your context window"

> "With extended thinking enabled, the Messages API for Claude 4 models returns a summary of Claude's full thinking process. Summarization is processed by a different model than the one you target in your requests."

> "Summarization preserves the key ideas of Claude's thinking process with minimal added latency"

### B. Текущая реализация `convert_to_tags`

```rust
ThinkingMode::ConvertToTags => {
    *item = serde_json::json!({
        "type": "text",
        "text": format!("<think>{}</think>", text),
    });
    result.tag_count = result.tag_count.saturating_add(1);
    changed = true;
}
```

Проблема: API видит это как обычный текст и не применяет thinking-специфичную логику.

### C. Ссылки

**Официальная документация:**
- [Anthropic Extended Thinking Docs](https://platform.claude.com/docs/en/build-with-claude/extended-thinking)
- [Extended Thinking with Tool Use (Cookbook)](https://github.com/anthropics/anthropic-cookbook/blob/main/extended_thinking/extended_thinking_with_tool_use.ipynb)
- [Context Editing Docs](https://platform.claude.com/docs/en/build-with-claude/context-editing)

**Унификация LLM провайдеров:**
- [one-api](https://github.com/songquanpeng/one-api) — Go, 29k stars, relay + adapters
- [LiteLLM](https://github.com/BerriAI/litellm) — Python, OpenAI-compatible wrapper
- [inference-gateway](https://github.com/inference-gateway/inference-gateway) — Go, unified proxy
- [llm-connector](https://github.com/lipish/llm-connector) — Rust, protocol/provider separation

**Проекты с обработкой thinking:**
- [LiteLLM issues #8961, #11302](https://github.com/BerriAI/litellm/issues/8961) — проблемы с signature
- [OpenCode issue #6176](https://github.com/anomalyco/opencode/issues/6176) — требования к thinking + tools
- [Antigravity-Manager](https://github.com/lbjlaq/Antigravity-Manager) — обработка при смене backend

**Архитектурные паттерны:**
- [tower-llm](https://docs.rs/tower-llm) — Rust, Tower-based LLM agents
- [async_trait](https://docs.rs/async-trait) — async методы в trait objects
