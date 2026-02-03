# Tool Context Preservation on Backend Switch

## Problem

При переключении бекенда текущая реализация суммаризации извлекает только текстовые блоки (`type: "text"`). Блоки `tool_use` и `tool_result` полностью игнорируются.

Это приводит к потере важного контекста:
- Какие файлы читались и их содержимое
- Какие команды выполнялись и их результаты
- Какие инструменты использовались

## Research (2026-02-04)

### Эксперимент

Тестирование проводилось с переключениями GLM → Anthropic → GLM:

1. На GLM: запустили Explore agent, прочитали DESIGN.md (244 строки) через Read tool
2. Переключились на Anthropic
3. Переключились обратно на GLM
4. Попросили процитировать DESIGN.md "из памяти"

### Анализ debug.log

При переключении бекенда в `[SUMMARIZE]` запросе видим:

```json
"content": "Session history:\n[USER]\n...\n[/USER]\n\n[ASSISTANT]\n## DESIGN.md — краткое содержание\nДокумент описывает...\n[/ASSISTANT]"
```

**Ключевое наблюдение:** В саммари попадает только **текстовый пересказ** Claude о файле, а не полный `tool_result` с содержимым файла (244 строки).

### Результат

GLM смог "процитировать" DESIGN.md после переключений. Цитата выглядела правдоподобно и частично совпадала с оригиналом.

**Однако это галлюцинация:**
- Оригинальный файл: 244 строки с кодом, диаграммами, таблицами
- В саммари попало: краткое описание в несколько абзацев
- GLM **реконструировал** содержимое на основе:
  - Своего краткого пересказа
  - Общих паттернов (как выглядят design docs)
  - "Угадывания" структуры

То, что реконструкция частично совпала с оригиналом — случайность, а не доказательство сохранения контекста.

### Вывод

**Tool_result теряется при переключении бекенда.** Модель работает по "памяти" о том, что обсуждалось, а не по реальному содержимому файлов.

## Solution

Разделить обработку контекста на две части:

### 1. Текстовый контент → LLM суммаризация

Текстовые блоки проходят через LLM для создания краткого саммари:
- Диалог между пользователем и ассистентом
- Принятые решения
- Текущие задачи

### 2. Инструменты → Прямая передача (дедуплицированные)

Tool blocks передаются напрямую в первое сообщение на новом бекенде:
- Без суммаризации LLM
- С дедупликацией (только последнее состояние)

## Deduplication Rules

### Read Tool
- Ключ: `file_path`
- Логика: оставлять только последний `tool_result` для каждого уникального `file_path`
- Причина: старые чтения файла неактуальны, важно только последнее состояние

### Bash Tool
- Ключ: `command` (точное совпадение)
- Логика: оставлять только последний результат для идентичных команд
- Исключение: команды с side effects (git commit, rm, etc.) — оставлять все

### Edit/Write Tools
- Ключ: `file_path`
- Логика: оставлять только последнюю операцию для каждого файла
- Причина: важен финальный результат редактирования

### Other Tools
- По умолчанию: оставлять все (без дедупликации)
- Можно добавить специфичные правила по мере необходимости

## Message Format

При переключении бекенда первое сообщение будет содержать:

```
[CONTEXT FROM PREVIOUS SESSION]

## Summary
{LLM-generated summary of text conversation}

## Files Read
### /path/to/file.rs
```rust
{actual file content from last tool_result}
```

## Commands Executed
### cargo test
```
{actual output from last tool_result}
```

[/CONTEXT FROM PREVIOUS SESSION]

{Original user message}
```

## Implementation Plan

### Step 1: Extract Tool Blocks

Файл: `src/proxy/thinking/summarizer.rs`

1. Добавить функцию `extract_tool_blocks(messages: &[Value]) -> Vec<ToolBlock>`
2. Структура `ToolBlock`:
   ```rust
   struct ToolBlock {
       tool_use: Value,      // original tool_use block
       tool_result: Value,   // corresponding tool_result
       tool_name: String,    // "Read", "Bash", etc.
       dedup_key: String,    // key for deduplication
   }
   ```

### Step 2: Deduplication

1. Добавить функцию `deduplicate_tools(blocks: Vec<ToolBlock>) -> Vec<ToolBlock>`
2. Реализовать логику по tool_name:
   - Read: dedup by file_path
   - Bash: dedup by command (with exceptions)
   - Edit/Write: dedup by file_path
   - Others: keep all

### Step 3: Format Tool Context

1. Добавить функцию `format_tool_context(blocks: &[ToolBlock]) -> String`
2. Формат вывода — человекочитаемый markdown с содержимым файлов и выводом команд

### Step 4: Integration

1. Обновить `SummarizeTransformer::on_backend_switch()`:
   - Извлечь tool blocks
   - Дедуплицировать
   - Форматировать
   - Объединить с текстовым саммари

2. Обновить `prepend_summary_to_user_message()`:
   - Включить tool context в prepend

## Size Considerations

Tool results могут быть большими. Стратегии ограничения:

1. **Truncation**: обрезать tool_result до N символов (например, 2000)
2. **Prioritization**: при превышении лимита — приоритет последним инструментам
3. **Config**: `max_tool_context_chars` в `[thinking.summarize]`

## Testing

1. Unit tests для дедупликации
2. Unit tests для форматирования
3. Integration test: полный цикл с tool_use/tool_result
4. Edge cases:
   - Пустой tool_result
   - Очень большой tool_result
   - Множественные чтения одного файла
   - Mix разных инструментов

## Open Questions

1. Нужно ли включать `tool_use` без соответствующего `tool_result`? (прерванные вызовы)
2. Как обрабатывать ошибки инструментов? (is_error: true)
3. Нужна ли конфигурация для включения/выключения tool context?
