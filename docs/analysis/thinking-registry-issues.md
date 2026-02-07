# Детальный анализ реализации ThinkingRegistry

## Найденные проблемы

### 1. Race Condition: Регистрация SSE vs Filter Request ⚠️ КРИТИЧНО

**Локация:** `src/proxy/upstream.rs:559-575`

**Описание:**
SSE streaming response обрабатывается в асинхронной задаче (tokio::spawn). Регистрация thinking blocks происходит отложенно, что создаёт race condition.

**Сценарий:**
```
1. Запрос 1: Filter request → нет блоков (registry пуст)
2. Бэкенд отвечает SSE → spawn задача начинает регистрацию
3. Запрос 2: Filter request → блоки ещё не зарегистрированы!
4. Блоки из запроса 1 фильтруются как "чужие"
5. Spawn задача завершает регистрацию (слишком поздно)
```

**Потенциальное решение:**
- Синхронная регистрация SSE блоков
- Или буферизация с гарантированной доставкой
- Или await на завершение регистрации перед следующим запросом

---

### 2. Race Condition: Backend Switch между запросами ⚠️ КРИТИЧНО

**Локация:** `src/proxy/upstream.rs:189-191`

**Описание:**
`notify_backend` вызывается при обработке запроса, но регистрация ответа происходит позже. Если между запросами был switch, блоки регистрируются в неправильной сессии.

**Сценарий:**
```
1. Запрос A: notify_backend(glm) → session=1
2. Запрос B: notify_backend(anthropic) → session=2 (switch!)
3. Запрос A получает ответ → регистрирует блоки в session=2 (wrong!)
4. Запрос B фильтрует блоки от запроса A (они в "неправильной" сессии)
```

**Потенциальное решение:**
- Запоминать session ID при обработке запроса
- Использовать сохранённый session ID при регистрации ответа
- Или привязать регистрацию к конкретному запросу (request_id)

---

### 3. Проблема: Неправильный порядок операций ⚠️ СЕРЬЁЗНО

**Локация:** `src/proxy/upstream.rs:198-211`

**Описание:**
Сначала выполняется filter_thinking_blocks, потом transform_request. Summarize transformer работает с уже отфильтрованным телом.

**Сценарий:**
```
1. Запрос содержит thinking blocks от GLM
2. Filter удаляет их (правильно)
3. Transform (summarize) работает с уже отфильтрованным телом
4. Summarize transformer сохраняет сообщения без thinking blocks
5. При switch создаётся summary без thinking context
```

**Вопрос:**
Это intentional behavior? Или summarize должен видеть оригинальные сообщения для создания полноценного summary?

**Потенциальное решение:**
- Вызвать summarize ДО filter
- Или передавать оригинальное тело в summarize отдельно

---

### 4. Race Condition: Filter + Register одновременно ⚠️ СЕРЬЁЗНО

**Локация:** `src/proxy/thinking/mod.rs:188-220`

**Описание:**
Между `filter_thinking_blocks` и `register_thinking_from_response` может произойти backend switch.

**Сценарий:**
```
// Запрос 1
registry.filter_thinking_blocks(&mut body); // session=1
// ... бэкенд обрабатывает ...
// Switch происходит здесь!
registry.register_thinking_from_response(&response); // session=2 (wrong!)
```

**Потенциальное решение:**
- Атомарные операции
- Или привязка session ID к запросу
- Или lock на весь цикл request-response

---

### 5. Проблема: Grace period для orphan блоков ⚠️ МИНОРНО

**Локация:** `src/proxy/thinking/registry.rs:26`

**Описание:**
5 минут grace period для unconfirmed блоков. При активном использовании Claude Code это нормально, но если пользователь сделает паузу на 5+ минут, потом отправит запрос — блоки будут удалены как orphaned.

**Вопрос:**
Это acceptable trade-off? Или grace period нужно сделать адаптивным?

**Потенциальное решение:**
- Уменьшить до 1-2 минут
- Или сделать адаптивным (зависит от активности)
- Или убрать grace period полностью (только confirmed/unconfirmed)

---

### 6. Проблема: Нет обработки ошибок при filter ⚠️ МИНОРНО

**Локация:** `src/proxy/upstream.rs:208-210`

**Описание:**
Если сериализация падает, мы молча игнорируем отфильтрованное тело и отправляем оригинал (с "чужими" thinking blocks).

**Код:**
```rust
if let Ok(updated) = serde_json::to_vec(&json_body) {
    body_bytes = updated;
}
// Если Err — молча продолжаем с оригинальным body_bytes
```

**Потенциальное решение:**
```rust
match serde_json::to_vec(&json_body) {
    Ok(updated) => body_bytes = updated,
    Err(e) => {
        tracing::error!(error = %e, "Failed to serialize filtered body");
        // Или вернуть ошибку
    }
}
```

---

### 7. Проблема: Transformer mode update ⚠️ МИНОРНО

**Локация:** `src/proxy/upstream.rs:194-196`

**Описание:**
`update_mode` вызывается при КАЖДОМ запросе, хотя mode меняется редко. Лишняя работа.

**Код:**
```rust
self.transformer_registry
    .update_mode(self.config.get().thinking.mode.clone())
    .await;
```

**Потенциальное решение:**
- Проверять, изменился ли mode, перед обновлением
- Или обновлять только при hot-reload конфига
- Или использовать watch/channel для изменений

---

## Резюме

| Проблема | Severity | Локация | Статус |
|----------|----------|---------|--------|
| SSE registration race | **Критично** | upstream.rs:559-575 | Не исправлено |
| Backend switch race | **Критично** | upstream.rs:189-191 | Не исправлено |
| Filter before transform | **Серьёзно** | upstream.rs:198-211 | Требует уточнения |
| Filter/Register race | **Серьёзно** | thinking/mod.rs:188-220 | Не исправлено |
| Grace period | Минорно | registry.rs:26 | Требует уточнения |
| Serialization error | Минорно | upstream.rs:208-210 | Не исправлено |
| Mode update | Минорно | upstream.rs:194-196 | Не исправлено |

## Рекомендации

1. **Приоритет 1 (Критично):** Исправить race conditions с SSE и backend switch
2. **Приоритет 2 (Серьёзно):** Уточнить порядок filter/transform и устранить race condition
3. **Приоритет 3 (Минорно):** Добавить обработку ошибок и оптимизировать mode update

---

*Анализ проведён: 2026-02-05*

---

## Ревью анализа (2026-02-05)

### По пункту 1: SSE Registration Race

**Статус:** Частично валидно, но низкий риск

**Анализ:**
- `on_complete` callback вызывается когда SSE stream **полностью завершён**
- К этому моменту Claude Code уже получил весь ответ
- CC работает последовательно: ждёт ответ → отправляет следующий запрос
- Race возможен теоретически, но на практике CC не отправит запрос до завершения stream

**Решение:** Документировать как known limitation. Не критично для CC.

---

### По пункту 2: Backend Switch Race

**Статус:** Валидно, но редкий кейс

**Анализ:**
- Требует конкурентных запросов к разным бэкендам
- CC работает последовательно, не отправляет параллельные запросы
- В реальном использовании этот race практически невозможен

**Решение:** Документировать. Если появятся другие клиенты — пересмотреть.

---

### По пункту 3: Filter before Transform

**Статус:** ❌ НЕ ПРОБЛЕМА — intentional behavior

**Анализ:**
- Filter удаляет thinking blocks с невалидными подписями
- Summarize работает с чистым телом — это **правильно**
- Summarize не должен сохранять thinking blocks (они бесполезны для другого бэкенда)
- Порядок операций корректен

**Решение:** Закрыть. Работает как задумано.

---

### По пункту 4: Filter/Register Race

**Статус:** = Пункт 2

**Анализ:** Тот же race condition, та же причина (конкурентные запросы).

---

### По пункту 5: Grace Period

**Статус:** ❌ НЕВЕРНЫЙ АНАЛИЗ в документе

**Анализ документа утверждает:**
> если пользователь сделает паузу на 5+ минут, потом отправит запрос — блоки будут удалены

**Реальный порядок в коде (`filter_request`):**
```
1. CONFIRM: блок в запросе → confirmed = true
2. CLEANUP: проверка orphan (только для unconfirmed!)
3. FILTER: оставляем блоки из кэша
```

**Важно:** Confirm происходит **ДО** cleanup. Если блок в запросе — он сначала подтверждается, а потом проверяется orphan rule. Подтверждённый блок **не удаляется** по orphan rule.

**Сценарий из документа:**
```
User AFK 5 min → Request с [A]
  1. Confirm: A ∈ request → A.confirmed = true
  2. Cleanup: A confirmed → orphan rule НЕ применяется
  3. Filter: A в кэше → оставляем
```

**Решение:** Закрыть. Анализ в документе ошибочен.

---

### По пункту 6: Serialization Error

**Статус:** ✅ ИСПРАВЛЕНО

**Было:**
```rust
if let Ok(updated) = serde_json::to_vec(&json_body) {
    body_bytes = updated;
}
// Ошибка молча игнорировалась
```

**Стало:**
```rust
match serde_json::to_vec(&json_body) {
    Ok(updated) => body_bytes = updated,
    Err(e) => {
        tracing::error!(
            error = %e,
            filtered_blocks = filtered,
            "Failed to serialize filtered request body, using original"
        );
    }
}
```

**Коммит:** 2026-02-05

---

### По пункту 7: Mode Update

**Статус:** ❌ УЖЕ ОПТИМИЗИРОВАНО

**Код `update_mode`:**
```rust
pub async fn update_mode(&self, mode: ThinkingMode) {
    let new_config = {
        let mut current_config = self.config.write()...;
        if current_config.mode != mode {
            // ... работа только если изменилось
            Some(current_config.clone())
        } else {
            None  // Быстрый выход, никакой работы
        }
    };
    // Async работа только если new_config.is_some()
}
```

Overhead при неизменном mode: один read lock (дёшево).

**Решение:** Закрыть. Уже оптимизировано.

---

## Обновлённая таблица

| Проблема | Severity | Статус |
|----------|----------|--------|
| SSE registration race | Низкий | Known limitation |
| Backend switch race | Низкий | Known limitation |
| Filter before transform | — | ❌ Не проблема |
| Filter/Register race | Низкий | = #2 |
| Grace period | — | ❌ Неверный анализ |
| Serialization error | Минорно | ✅ Исправлено |
| Mode update | — | ❌ Уже оптимизировано |

*Ревью проведено: 2026-02-05*
