# Pluggable Feature Architecture: Пошаговый план реализации

## 1. Обзор

### Что делаем

Рефакторинг ClaudeWrapper (AnyClaude) из монолитной архитектуры в pluggable feature-based архитектуру с использованием Cargo feature flags. Это позволяет:

1. **Добавлять новые фичи** (например, Claude Code version monitor) **без модификации** `App`, `draw()`, `classify_key()`, `runtime.rs`
2. **Параллельная разработка** -- каждая фича изолирована в своем модуле
3. **Безопасная миграция** -- оба пути (`cargo build` и `cargo build --features pluggable`) компилируются на каждом шаге

### Стратегия feature flag

```
cargo build                      # Монолитный путь (текущий код, без изменений)
cargo build --features pluggable # Pluggable путь (Feature trait + FeatureRegistry)
```

Оба пути **обязаны** компилироваться чисто с `dead_code = "deny"` и `unused_imports = "deny"` на каждом шаге.

Финальная цель: убрать монолитный путь и сделать `pluggable` дефолтом.

---

## 2. Сравнение архитектур

### Текущая (монолитная)

```
src/ui/app.rs      -- App struct (23 поля), все фичи живут как поля
src/ui/render.rs   -- draw() -- god function, hardcoded match PopupKind
src/ui/input.rs    -- classify_key() -- hardcoded hotkeys
src/ui/runtime.rs  -- run() -- hardcoded tick intervals и event handling
src/ui/events.rs   -- AppEvent enum -- hardcoded variants
```

**Добавить новую фичу** = модифицировать 6-8 файлов.

### Целевая (pluggable)

```
src/ui/feature.rs  -- Feature trait, FeatureContext, FeatureRegistry
src/features/      -- Каждая фича в своем файле, реализует Feature trait
src/ui/app.rs      -- App хранит FeatureRegistry вместо feature-полей
src/ui/render.rs   -- draw() делегирует registry.render_active_popup()
src/ui/input.rs    -- classify_key() делегирует registry.handle_key()
src/ui/runtime.rs  -- tick делегирует registry.tick()
```

**Добавить новую фичу** = создать 1 файл + 1 строку регистрации.

---

## 3. Core Traits & Types

### 3.1 Feature trait

```rust
// src/ui/feature.rs

use crate::config::ConfigStore;
use crate::error::ErrorRegistry;
use crate::ipc::{BackendInfo, ProxyStatus};
use crate::metrics::MetricsSnapshot;
use crate::ui::app::UiCommandSender;
use crate::ui::input::InputAction;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::Frame;
use term_input::KeyInput;

/// Уникальный идентификатор фичи (строковый, compile-time).
pub type FeatureId = &'static str;

/// Комбинация клавиш для привязки хоткея.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum KeyCombo {
    /// Ctrl + символ (например, Ctrl+S)
    Ctrl(char),
}

/// Команда, которую фича может вернуть из on_tick/handle_input.
/// Runtime выполняет команды после получения.
#[derive(Debug)]
pub enum FeatureCommand {
    /// Отправить UiCommand в bridge
    SendUiCommand(crate::ui::app::UiCommand),
    /// Показать попап этой фичи
    ShowPopup,
    /// Закрыть текущий попап
    ClosePopup,
    /// Запросить выход из приложения
    RequestQuit,
}

/// Событие жизненного цикла для фич.
#[derive(Debug, Clone)]
pub enum FeatureEvent {
    /// PTY стал Ready
    PtyReady,
    /// Конфиг перезагружен
    ConfigReload,
    /// Получен IPC статус
    IpcStatus(ProxyStatus),
    /// Получен список бэкендов
    IpcBackends(Vec<BackendInfo>),
    /// IPC ошибка
    IpcError(String),
}

/// Контекст, доступный фичам для чтения состояния приложения.
pub struct FeatureContext<'a> {
    pub config: &'a ConfigStore,
    pub error_registry: &'a ErrorRegistry,
    pub ipc_sender: Option<&'a UiCommandSender>,
    pub is_pty_ready: bool,
}

/// Контекст для рендеринга.
pub struct RenderContext<'a> {
    pub frame: &'a mut Frame<'a>,
    pub body: Rect,
}

/// Содержимое попапа, возвращаемое фичей для отрисовки.
pub struct PopupContent<'a> {
    pub title: &'a str,
    pub lines: Vec<Line<'a>>,
    pub footer: &'a str,
    pub min_width: u16,
    /// (total_items, scroll_offset) для скроллбара, None если не нужен.
    pub scrollbar: Option<(usize, usize)>,
}

/// Pluggable UI фича.
///
/// Фичи НЕ async -- они возвращают команды, runtime их выполняет.
pub trait Feature: Send + 'static {
    /// Уникальный идентификатор.
    fn id(&self) -> FeatureId;

    /// Хоткей для открытия попапа (None если нет попапа).
    fn hotkey(&self) -> Option<KeyCombo> {
        None
    }

    /// Инициализация фичи. Вызывается один раз при регистрации.
    fn init(&mut self, _ctx: &FeatureContext<'_>) {}

    /// Обработка тика (250ms). Возвращает команды для выполнения.
    fn on_tick(&mut self, _ctx: &FeatureContext<'_>) -> Vec<FeatureCommand> {
        Vec::new()
    }

    /// Обработка события жизненного цикла.
    fn on_event(&mut self, _event: &FeatureEvent, _ctx: &FeatureContext<'_>) {}

    /// Обработка нажатия клавиши когда попап этой фичи активен.
    /// Возвращает `None` если клавиша не обработана (передать дальше).
    fn handle_popup_key(
        &mut self,
        _key: &KeyInput,
        _ctx: &FeatureContext<'_>,
    ) -> Option<Vec<FeatureCommand>> {
        None
    }

    /// Возвращает содержимое попапа для отрисовки.
    /// None если попап не должен быть показан (Hidden state).
    fn popup_content(&self) -> Option<PopupContent<'_>> {
        None
    }

    /// Рендерит попап, полностью контролируя отрисовку.
    /// Используется фичами с кастомным рендерингом (History, Settings).
    /// Возвращает true если фича отрисовала себя сама (PopupDialog не нужен).
    fn render_custom(&self, _frame: &mut Frame<'_>, _body: Rect) -> bool {
        false
    }

    /// Дополнительные спаны для footer (версия и т.д.).
    fn footer_spans(&self) -> Vec<ratatui::text::Span<'static>> {
        Vec::new()
    }
}
```

### 3.2 FeatureRegistry

```rust
// Продолжение src/ui/feature.rs

use crate::ui::components::PopupDialog;

/// Реестр всех pluggable фич.
pub struct FeatureRegistry {
    /// Зарегистрированные фичи (порядок = порядок регистрации).
    features: Vec<Box<dyn Feature>>,
    /// Маппинг хоткей -> индекс в features.
    hotkey_map: Vec<(KeyCombo, usize)>,
    /// Текущий активный попап (индекс в features), None если нет.
    active_popup: Option<usize>,
}

impl FeatureRegistry {
    pub fn new() -> Self {
        Self {
            features: Vec::new(),
            hotkey_map: Vec::new(),
            active_popup: None,
        }
    }

    /// Зарегистрировать фичу. Вызывать при инициализации.
    pub fn register(&mut self, feature: Box<dyn Feature>) {
        let idx = self.features.len();
        if let Some(hotkey) = feature.hotkey() {
            self.hotkey_map.push((hotkey, idx));
        }
        self.features.push(feature);
    }

    /// Инициализировать все фичи.
    pub fn init_all(&mut self, ctx: &FeatureContext<'_>) {
        for feature in &mut self.features {
            feature.init(ctx);
        }
    }

    /// Есть ли активный попап?
    pub fn has_active_popup(&self) -> bool {
        self.active_popup.is_some()
    }

    /// ID активного попапа.
    pub fn active_popup_id(&self) -> Option<FeatureId> {
        self.active_popup
            .and_then(|idx| self.features.get(idx))
            .map(|f| f.id())
    }

    /// Закрыть активный попап.
    pub fn close_popup(&mut self) {
        self.active_popup = None;
    }

    /// Открыть попап по feature ID.
    pub fn open_popup(&mut self, feature_id: FeatureId) {
        if let Some(idx) = self.features.iter().position(|f| f.id() == feature_id) {
            self.active_popup = Some(idx);
        }
    }

    /// Переключить попап (toggle). Возвращает true если попап открыт.
    pub fn toggle_popup(&mut self, feature_id: FeatureId) -> bool {
        if self.active_popup_id() == Some(feature_id) {
            self.close_popup();
            false
        } else {
            self.open_popup(feature_id);
            true
        }
    }

    /// Проверить хоткей. Возвращает Some(feature_id) если это хоткей фичи.
    pub fn match_hotkey(&self, key: &KeyInput) -> Option<FeatureId> {
        let combo = match &key.kind {
            term_input::KeyKind::Control(ch) => KeyCombo::Ctrl(*ch),
            _ => return None,
        };
        self.hotkey_map
            .iter()
            .find(|(k, _)| *k == combo)
            .and_then(|(_, idx)| self.features.get(*idx))
            .map(|f| f.id())
    }

    /// Обработать клавишу в активном попапе.
    /// Возвращает Some(commands) если клавиша обработана, None если нет.
    pub fn handle_popup_key(
        &mut self,
        key: &KeyInput,
        ctx: &FeatureContext<'_>,
    ) -> Option<Vec<FeatureCommand>> {
        let idx = self.active_popup?;
        let feature = self.features.get_mut(idx)?;
        feature.handle_popup_key(key, ctx)
    }

    /// Тик для всех фич. Возвращает собранные команды.
    pub fn tick(&mut self, ctx: &FeatureContext<'_>) -> Vec<FeatureCommand> {
        let mut commands = Vec::new();
        for feature in &mut self.features {
            commands.extend(feature.on_tick(ctx));
        }
        commands
    }

    /// Отправить событие всем фичам.
    pub fn broadcast_event(&mut self, event: &FeatureEvent, ctx: &FeatureContext<'_>) {
        for feature in &mut self.features {
            feature.on_event(event, ctx);
        }
    }

    /// Отрисовать активный попап.
    pub fn render_active_popup(&self, frame: &mut Frame<'_>, body: Rect) {
        let Some(idx) = self.active_popup else {
            return;
        };
        let Some(feature) = self.features.get(idx) else {
            return;
        };

        // Сначала пробуем кастомный рендеринг
        if feature.render_custom(frame, body) {
            return;
        }

        // Стандартный рендеринг через PopupDialog
        if let Some(content) = feature.popup_content() {
            let mut dialog = PopupDialog::new(content.title, content.lines)
                .footer(content.footer)
                .min_width(content.min_width);
            if let Some((total, offset)) = content.scrollbar {
                dialog = dialog.scrollbar(total, offset);
            }
            dialog.render(frame, body);
        }
    }

    /// Собрать footer спаны от всех фич.
    pub fn footer_spans(&self) -> Vec<ratatui::text::Span<'static>> {
        let mut spans = Vec::new();
        for feature in &self.features {
            spans.extend(feature.footer_spans());
        }
        spans
    }

    /// Получить мутабельную ссылку на фичу по ID.
    pub fn get_mut(&mut self, id: FeatureId) -> Option<&mut dyn Feature> {
        self.features
            .iter_mut()
            .find(|f| f.id() == id)
            .map(|f| f.as_mut())
    }

    /// Получить ссылку на фичу по ID.
    pub fn get(&self, id: FeatureId) -> Option<&dyn Feature> {
        self.features
            .iter()
            .find(|f| f.id() == id)
            .map(|f| f.as_ref())
    }

    /// Выполнить команды и вернуть те, которые требуют обработки runtime.
    pub fn execute_commands(&mut self, commands: Vec<FeatureCommand>) -> Vec<FeatureCommand> {
        let mut external = Vec::new();
        for cmd in commands {
            match cmd {
                FeatureCommand::ShowPopup => {
                    // Уже обрабатывается в handle_key
                }
                FeatureCommand::ClosePopup => {
                    self.close_popup();
                }
                other => external.push(other),
            }
        }
        external
    }
}
```

---

## 4. Пошаговый план реализации

---

### Phase 1: Foundation (чисто аддитивные изменения, нулевые модификации существующего кода)

---

#### Шаг 1.1: Добавить feature flag `pluggable` в Cargo.toml

**Цель**: Определить feature flag, который позволит условно компилировать новый код.

**Файлы для модификации**:

**`Cargo.toml`** (строка 14-15):

ДО:
```toml
[features]
default = []
```

ПОСЛЕ:
```toml
[features]
default = []
pluggable = []
```

**Проверка**:
```bash
cargo build                      # OK (ничего не изменилось)
cargo build --features pluggable # OK (пустой feature, ничего не делает)
```

**Откат**: Удалить строку `pluggable = []`.

---

#### Шаг 1.2: Создать `src/ui/feature.rs` -- core traits и registry (за cfg)

**Цель**: Создать файл с трейтом `Feature`, `FeatureRegistry`, `FeatureContext` и всеми вспомогательными типами. Весь файл за `#[cfg(feature = "pluggable")]`.

**Файлы для создания**:

**`src/ui/feature.rs`** (новый файл):

```rust
//! Pluggable feature architecture.
//!
//! Предоставляет трейт `Feature` и `FeatureRegistry` для plug-in
//! UI фич без модификации core файлов (app.rs, render.rs, input.rs).

#![cfg(feature = "pluggable")]

use crate::config::ConfigStore;
use crate::error::ErrorRegistry;
use crate::ipc::{BackendInfo, ProxyStatus};
use crate::metrics::MetricsSnapshot;
use crate::ui::app::UiCommandSender;
use crate::ui::components::PopupDialog;
use crate::ui::input::InputAction;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::Frame;
use term_input::KeyInput;

// ─── Types ───────────────────────────────────────────────────────────

/// Уникальный идентификатор фичи.
pub type FeatureId = &'static str;

/// Комбинация клавиш для привязки хоткея.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum KeyCombo {
    /// Ctrl + символ.
    Ctrl(char),
}

/// Команда от фичи к runtime.
#[derive(Debug)]
pub enum FeatureCommand {
    /// Отправить UiCommand в bridge.
    SendUiCommand(crate::ui::app::UiCommand),
    /// Показать попап этой фичи.
    ShowPopup,
    /// Закрыть текущий попап.
    ClosePopup,
    /// Запросить выход из приложения.
    RequestQuit,
}

/// Событие жизненного цикла.
#[derive(Debug, Clone)]
pub enum FeatureEvent {
    /// PTY стал Ready.
    PtyReady,
    /// Конфиг перезагружен.
    ConfigReload,
    /// IPC статус получен.
    IpcStatus(ProxyStatus),
    /// Список бэкендов получен.
    IpcBackends(Vec<BackendInfo>),
    /// IPC ошибка.
    IpcError(String),
}

/// Read-only контекст для фич.
pub struct FeatureContext<'a> {
    pub config: &'a ConfigStore,
    pub error_registry: &'a ErrorRegistry,
    pub ipc_sender: Option<&'a UiCommandSender>,
    pub is_pty_ready: bool,
}

/// Содержимое попапа для отрисовки через PopupDialog.
pub struct PopupContent<'a> {
    pub title: &'a str,
    pub lines: Vec<Line<'a>>,
    pub footer: &'a str,
    pub min_width: u16,
    pub scrollbar: Option<(usize, usize)>,
}

// ─── Feature Trait ───────────────────────────────────────────────────

/// Pluggable UI фича.
///
/// Фичи синхронные -- возвращают команды, runtime выполняет.
pub trait Feature: Send + 'static {
    /// Уникальный идентификатор.
    fn id(&self) -> FeatureId;

    /// Хоткей для toggle попапа. None = нет попапа.
    fn hotkey(&self) -> Option<KeyCombo> {
        None
    }

    /// Инициализация. Вызывается один раз при регистрации.
    fn init(&mut self, _ctx: &FeatureContext<'_>) {}

    /// Тик (250ms). Возвращает команды.
    fn on_tick(&mut self, _ctx: &FeatureContext<'_>) -> Vec<FeatureCommand> {
        Vec::new()
    }

    /// Событие жизненного цикла.
    fn on_event(&mut self, _event: &FeatureEvent, _ctx: &FeatureContext<'_>) {}

    /// Обработка клавиши в активном попапе.
    /// None = клавиша не обработана.
    fn handle_popup_key(
        &mut self,
        _key: &KeyInput,
        _ctx: &FeatureContext<'_>,
    ) -> Option<Vec<FeatureCommand>> {
        None
    }

    /// Содержимое попапа. None = попап скрыт.
    fn popup_content(&self) -> Option<PopupContent<'_>> {
        None
    }

    /// Кастомный рендеринг. true = фича отрисовала себя сама.
    fn render_custom(&self, _frame: &mut Frame<'_>, _body: Rect) -> bool {
        false
    }

    /// Спаны для footer.
    fn footer_spans(&self) -> Vec<ratatui::text::Span<'static>> {
        Vec::new()
    }
}

// ─── FeatureRegistry ────────────────────────────────────────────────

/// Реестр pluggable фич.
pub struct FeatureRegistry {
    features: Vec<Box<dyn Feature>>,
    hotkey_map: Vec<(KeyCombo, usize)>,
    active_popup: Option<usize>,
}

impl FeatureRegistry {
    pub fn new() -> Self {
        Self {
            features: Vec::new(),
            hotkey_map: Vec::new(),
            active_popup: None,
        }
    }

    /// Зарегистрировать фичу.
    pub fn register(&mut self, feature: Box<dyn Feature>) {
        let idx = self.features.len();
        if let Some(hotkey) = feature.hotkey() {
            self.hotkey_map.push((hotkey, idx));
        }
        self.features.push(feature);
    }

    /// Инициализировать все фичи.
    pub fn init_all(&mut self, ctx: &FeatureContext<'_>) {
        for feature in &mut self.features {
            feature.init(ctx);
        }
    }

    /// Есть ли активный попап?
    pub fn has_active_popup(&self) -> bool {
        self.active_popup.is_some()
    }

    /// ID активного попапа.
    pub fn active_popup_id(&self) -> Option<FeatureId> {
        self.active_popup
            .and_then(|idx| self.features.get(idx))
            .map(|f| f.id())
    }

    /// Закрыть попап.
    pub fn close_popup(&mut self) {
        self.active_popup = None;
    }

    /// Открыть попап по feature ID.
    pub fn open_popup(&mut self, feature_id: FeatureId) {
        if let Some(idx) = self.features.iter().position(|f| f.id() == feature_id) {
            self.active_popup = Some(idx);
        }
    }

    /// Toggle попап. Возвращает true если открыт.
    pub fn toggle_popup(&mut self, feature_id: FeatureId) -> bool {
        if self.active_popup_id() == Some(feature_id) {
            self.close_popup();
            false
        } else {
            self.open_popup(feature_id);
            true
        }
    }

    /// Проверить хоткей. Возвращает feature_id если совпало.
    pub fn match_hotkey(&self, key: &KeyInput) -> Option<FeatureId> {
        let combo = match &key.kind {
            term_input::KeyKind::Control(ch) => KeyCombo::Ctrl(*ch),
            _ => return None,
        };
        self.hotkey_map
            .iter()
            .find(|(k, _)| *k == combo)
            .and_then(|(_, idx)| self.features.get(*idx))
            .map(|f| f.id())
    }

    /// Обработать клавишу в активном попапе.
    pub fn handle_popup_key(
        &mut self,
        key: &KeyInput,
        ctx: &FeatureContext<'_>,
    ) -> Option<Vec<FeatureCommand>> {
        let idx = self.active_popup?;
        let feature = self.features.get_mut(idx)?;
        feature.handle_popup_key(key, ctx)
    }

    /// Тик для всех фич.
    pub fn tick(&mut self, ctx: &FeatureContext<'_>) -> Vec<FeatureCommand> {
        let mut commands = Vec::new();
        for feature in &mut self.features {
            commands.extend(feature.on_tick(ctx));
        }
        commands
    }

    /// Broadcast событие всем фичам.
    pub fn broadcast_event(&mut self, event: &FeatureEvent, ctx: &FeatureContext<'_>) {
        for feature in &mut self.features {
            feature.on_event(event, ctx);
        }
    }

    /// Отрисовать активный попап.
    pub fn render_active_popup(&self, frame: &mut Frame<'_>, body: Rect) {
        let Some(idx) = self.active_popup else {
            return;
        };
        let Some(feature) = self.features.get(idx) else {
            return;
        };

        // Кастомный рендеринг
        if feature.render_custom(frame, body) {
            return;
        }

        // Стандартный через PopupDialog
        if let Some(content) = feature.popup_content() {
            let mut dialog = PopupDialog::new(content.title, content.lines)
                .footer(content.footer)
                .min_width(content.min_width);
            if let Some((total, offset)) = content.scrollbar {
                dialog = dialog.scrollbar(total, offset);
            }
            dialog.render(frame, body);
        }
    }

    /// Footer спаны от всех фич.
    pub fn footer_spans(&self) -> Vec<ratatui::text::Span<'static>> {
        let mut spans = Vec::new();
        for feature in &self.features {
            spans.extend(feature.footer_spans());
        }
        spans
    }

    /// Мутабельная ссылка на фичу.
    pub fn get_mut(&mut self, id: FeatureId) -> Option<&mut dyn Feature> {
        self.features
            .iter_mut()
            .find(|f| f.id() == id)
            .map(|f| f.as_mut())
    }

    /// Ссылка на фичу.
    pub fn get(&self, id: FeatureId) -> Option<&dyn Feature> {
        self.features
            .iter()
            .find(|f| f.id() == id)
            .map(|f| f.as_ref())
    }

    /// Выполнить internal команды, вернуть external.
    pub fn execute_commands(&mut self, commands: Vec<FeatureCommand>) -> Vec<FeatureCommand> {
        let mut external = Vec::new();
        for cmd in commands {
            match cmd {
                FeatureCommand::ShowPopup => {
                    // Обрабатывается в вызывающем коде
                }
                FeatureCommand::ClosePopup => {
                    self.close_popup();
                }
                other => external.push(other),
            }
        }
        external
    }
}
```

**Проверка**:
```bash
cargo build                      # OK (файл за cfg, не компилируется)
cargo build --features pluggable # OK (типы определены, нигде не используются)
```

**ВАЖНО**: `dead_code = "deny"` не срабатывает на типы за `cfg` feature, который enabled -- НО на этом этапе типы ещё нигде не используются. Нужно добавить `#[allow(dead_code)]` **временно** на весь модуль до Phase 2.

Фактический код на этом шаге будет с атрибутом:
```rust
// В начале файла после #![cfg(feature = "pluggable")]
#![allow(dead_code)] // SAFETY: removed in Phase 2 when features start using this
```

Нет -- `#![allow]` inner attributes работают только в crate root. Вместо этого используем:
```rust
#[cfg(feature = "pluggable")]
#[allow(dead_code)] // Temporary: removed in Step 2.1 when StatusFeature uses these types
pub mod feature;
```
в `src/ui/mod.rs` (шаг 1.4).

**Откат**: `rm src/ui/feature.rs`

---

#### Шаг 1.3: Создать `src/features/mod.rs` -- корневой модуль фич (за cfg)

**Цель**: Создать директорию и корневой модуль для pluggable фич.

**Файлы для создания**:

**`src/features/mod.rs`** (новый файл):

```rust
//! Pluggable features.
//!
//! Каждый подмодуль реализует `Feature` trait и регистрируется
//! в `FeatureRegistry` при инициализации.

use crate::ui::feature::FeatureRegistry;

/// Создать registry и зарегистрировать все фичи.
pub fn create_registry() -> FeatureRegistry {
    let registry = FeatureRegistry::new();
    // Фичи будут регистрироваться здесь в Phase 2+
    registry
}
```

**Проверка**:
```bash
cargo build --features pluggable # OK
```

**Откат**: `rm -rf src/features/`

---

#### Шаг 1.4: Подключить модули в `src/ui/mod.rs` и `src/lib.rs` (за cfg)

**Цель**: Подключить новые модули в дерево компиляции, но только при `--features pluggable`.

**Файлы для модификации**:

**`src/ui/mod.rs`** (строки 1-19):

ДО:
```rust
pub mod app;
pub mod events;
pub mod footer;
pub mod header;
pub mod history;
pub mod input;
pub mod layout;
pub mod components;
pub mod mvi;
pub mod pty;
pub mod render;
pub mod selection;
pub mod settings;
pub mod runtime;
pub mod terminal;
pub mod terminal_guard;
pub mod theme;

pub use runtime::run;
```

ПОСЛЕ:
```rust
pub mod app;
pub mod events;
#[cfg(feature = "pluggable")]
#[allow(dead_code)] // Temporary: removed when features use these types (Step 2.1)
pub mod feature;
pub mod footer;
pub mod header;
pub mod history;
pub mod input;
pub mod layout;
pub mod components;
pub mod mvi;
pub mod pty;
pub mod render;
pub mod selection;
pub mod settings;
pub mod runtime;
pub mod terminal;
pub mod terminal_guard;
pub mod theme;

pub use runtime::run;
```

**`src/lib.rs`** (строки 1-13):

ДО:
```rust
pub mod args;
pub mod backend;
pub mod clipboard;
pub mod config;
pub mod error;
pub mod ipc;
pub mod metrics;
pub mod proxy;
pub mod pty;
pub mod shim;
pub mod shutdown;
pub mod sse;
pub mod ui;
```

ПОСЛЕ:
```rust
pub mod args;
pub mod backend;
pub mod clipboard;
pub mod config;
pub mod error;
#[cfg(feature = "pluggable")]
#[allow(dead_code)] // Temporary: removed when features are registered (Step 2.1)
pub mod features;
pub mod ipc;
pub mod metrics;
pub mod proxy;
pub mod pty;
pub mod shim;
pub mod shutdown;
pub mod sse;
pub mod ui;
```

**Проверка**:
```bash
cargo build                      # OK (cfg не активен, модули не компилируются)
cargo build --features pluggable # OK (модули компилируются, dead_code разрешен)
cargo test                       # OK (существующие тесты проходят)
```

**Откат**: Убрать добавленные строки из `src/ui/mod.rs` и `src/lib.rs`.

---

### Phase 2: ~~Status Feature~~ (REMOVED)

> Network Diagnostics / Status popup was removed from the codebase.
> This phase is no longer applicable.

---

### Phase 3: BackendSwitch Feature Extraction

---

#### Шаг 3.1: Создать `src/features/backend_switch.rs`

**Цель**: Инкапсулировать BackendSwitch попап как `impl Feature`.

---

#### Шаг 3.1: Создать `src/features/backend_switch.rs`

**Цель**: Инкапсулировать BackendSwitch попап как `impl Feature`.

Данные из App:
- `backends: Vec<BackendInfo>` (строка 68)
- `backend_selection: usize` (строка 69)
- `last_backends_refresh: Instant` (строка 73)

Логика из:
- `render.rs` строки 233-289 (рендеринг)
- `input.rs` строки 126-207 (обработка клавиш)
- `app.rs` строки 360-476 (методы)

**Файлы для создания**:

**`src/features/backend_switch.rs`** (новый файл):

```rust
//! Backend switch popup feature.

use crate::ipc::BackendInfo;
use crate::ui::app::UiCommand;
use crate::ui::feature::{
    Feature, FeatureCommand, FeatureContext, FeatureEvent, FeatureId, KeyCombo, PopupContent,
};
use crate::ui::theme::{ACTIVE_HIGHLIGHT, HEADER_TEXT, STATUS_ERROR, STATUS_OK};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use std::time::{Duration, Instant};
use term_input::{Direction, KeyInput, KeyKind};

const BACKENDS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub struct BackendSwitchFeature {
    backends: Vec<BackendInfo>,
    selection: usize,
    last_refresh: Instant,
    last_ipc_error: Option<String>,
}

impl BackendSwitchFeature {
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
            selection: 0,
            last_refresh: Instant::now(),
            last_ipc_error: None,
        }
    }

    /// Доступ к списку бэкендов (для других фич).
    pub fn backends(&self) -> &[BackendInfo] {
        &self.backends
    }

    fn reset_selection(&mut self) {
        self.selection = self
            .backends
            .iter()
            .position(|b| b.is_active)
            .unwrap_or(0);
    }

    fn clamp_selection(&mut self) {
        if self.backends.is_empty() {
            self.selection = 0;
            return;
        }
        let max = self.backends.len() - 1;
        if self.selection > max {
            self.selection = max;
        }
    }

    fn move_selection(&mut self, direction: i32) {
        if self.backends.is_empty() {
            self.selection = 0;
            return;
        }
        let len = self.backends.len();
        let current = self.selection.min(len.saturating_sub(1));
        self.selection = if direction.is_negative() {
            if current == 0 { len - 1 } else { current - 1 }
        } else if current + 1 >= len {
            0
        } else {
            current + 1
        };
    }

    fn switch_by_index(&mut self, index: usize) -> Vec<FeatureCommand> {
        let Some(backend) = self.backends.get(index.saturating_sub(1)) else {
            return vec![];
        };
        if backend.is_active {
            return vec![FeatureCommand::ClosePopup];
        }
        let cmd = FeatureCommand::SendUiCommand(UiCommand::SwitchBackend {
            backend_id: backend.id.clone(),
        });
        vec![cmd, FeatureCommand::ClosePopup]
    }
}

impl Feature for BackendSwitchFeature {
    fn id(&self) -> FeatureId {
        "backend_switch"
    }

    fn hotkey(&self) -> Option<KeyCombo> {
        Some(KeyCombo::Ctrl('b'))
    }

    fn init(&mut self, _ctx: &FeatureContext<'_>) {}

    fn on_tick(&mut self, _ctx: &FeatureContext<'_>) -> Vec<FeatureCommand> {
        // Обновление бэкендов только когда попап открыт
        // (определяется в runtime по active_popup_id)
        vec![]
    }

    fn on_event(&mut self, event: &FeatureEvent, _ctx: &FeatureContext<'_>) {
        match event {
            FeatureEvent::IpcBackends(backends) => {
                let was_empty = self.backends.is_empty();
                self.backends = backends.clone();
                if was_empty {
                    self.reset_selection();
                } else {
                    self.clamp_selection();
                }
            }
            FeatureEvent::IpcError(msg) => {
                self.last_ipc_error = Some(msg.clone());
            }
            FeatureEvent::IpcStatus(_) => {
                self.last_ipc_error = None;
            }
            _ => {}
        }
    }

    fn handle_popup_key(
        &mut self,
        key: &KeyInput,
        _ctx: &FeatureContext<'_>,
    ) -> Option<Vec<FeatureCommand>> {
        match &key.kind {
            KeyKind::Escape | KeyKind::Control('b') => {
                Some(vec![FeatureCommand::ClosePopup])
            }
            KeyKind::Control('s') | KeyKind::Control('h') => {
                Some(vec![FeatureCommand::ClosePopup])
            }
            KeyKind::Arrow(Direction::Up) => {
                self.move_selection(-1);
                Some(vec![])
            }
            KeyKind::Arrow(Direction::Down) => {
                self.move_selection(1);
                Some(vec![])
            }
            KeyKind::Enter => {
                let index = self.selection;
                let Some(backend) = self.backends.get(index) else {
                    return Some(vec![]);
                };
                if backend.is_active {
                    return Some(vec![FeatureCommand::ClosePopup]);
                }
                Some(self.switch_by_index(index + 1))
            }
            KeyKind::Char(ch) if ch.is_ascii_digit() => {
                let index = ch.to_digit(10).unwrap_or(0) as usize;
                if index > 0 {
                    Some(self.switch_by_index(index))
                } else {
                    Some(vec![])
                }
            }
            _ => Some(vec![]),
        }
    }

    fn popup_content(&self) -> Option<PopupContent<'_>> {
        let mut lines = Vec::new();

        if self.backends.is_empty() {
            lines.push(Line::from("    No backends available."));
        } else {
            let max_name_width = self
                .backends
                .iter()
                .map(|b| b.display_name.chars().count())
                .max()
                .unwrap_or(0);

            for (idx, backend) in self.backends.iter().enumerate() {
                let (status_text, status_color) = if backend.is_active {
                    ("Active", STATUS_OK)
                } else if backend.is_configured {
                    ("Ready", STATUS_OK)
                } else {
                    ("Missing", STATUS_ERROR)
                };
                let is_selected = idx == self.selection;

                let base_style = if is_selected {
                    Style::default().bg(ACTIVE_HIGHLIGHT)
                } else {
                    Style::default()
                };

                let prefix = if is_selected {
                    format!("  \u{2192} {}. ", idx + 1)
                } else {
                    format!("    {}. ", idx + 1)
                };
                let spans = vec![
                    Span::styled(prefix, base_style.fg(HEADER_TEXT)),
                    Span::styled(
                        format!("{:<width$}", backend.display_name, width = max_name_width),
                        base_style.fg(HEADER_TEXT),
                    ),
                    Span::styled("  [", base_style),
                    Span::styled(status_text, base_style.fg(status_color)),
                    Span::styled("]", base_style),
                ];
                lines.push(Line::from(spans));
            }
        }

        if let Some(error) = &self.last_ipc_error {
            lines.push(Line::from(""));
            lines.push(Line::from(format!("    IPC error: {error}")));
        }

        Some(PopupContent {
            title: "Select Backend",
            lines,
            footer: "Up/Down: Move  Enter: Select  Esc/Ctrl+B: Close",
            min_width: 60,
            scrollbar: None,
        })
    }
}
```

**`src/features/mod.rs`** -- обновить:

```rust
pub mod backend_switch;
pub mod status;

use crate::ui::feature::FeatureRegistry;

pub fn create_registry() -> FeatureRegistry {
    let mut registry = FeatureRegistry::new();
    registry.register(Box::new(status::StatusFeature::new()));
    registry.register(Box::new(backend_switch::BackendSwitchFeature::new()));
    registry
}
```

**Проверка**:
```bash
cargo build --features pluggable
```

**Откат**: `rm src/features/backend_switch.rs`, откатить `mod.rs`.

---

#### Шаг 3.2: Интеграционный тест для BackendSwitchFeature

**`tests/backend_switch_feature.rs`** (новый файл):

```rust
#![cfg(feature = "pluggable")]

use anyclaude::config::{Config, ConfigStore};
use anyclaude::error::ErrorRegistry;
use anyclaude::ipc::BackendInfo;
use anyclaude::features::backend_switch::BackendSwitchFeature;
use anyclaude::ui::feature::{Feature, FeatureContext, FeatureEvent};
use std::path::PathBuf;

fn make_ctx() -> (ConfigStore, ErrorRegistry) {
    let config = ConfigStore::new(Config::default(), PathBuf::from("/tmp/test.toml"));
    let error_registry = ErrorRegistry::new(100);
    (config, error_registry)
}

fn make_backends() -> Vec<BackendInfo> {
    vec![
        BackendInfo {
            id: "anthropic".to_string(),
            display_name: "Anthropic".to_string(),
            is_active: true,
            is_configured: true,
            base_url: "https://api.anthropic.com".to_string(),
        },
        BackendInfo {
            id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            is_active: false,
            is_configured: true,
            base_url: "https://api.openai.com".to_string(),
        },
    ]
}

#[test]
fn backend_switch_feature_id() {
    let feature = BackendSwitchFeature::new();
    assert_eq!(feature.id(), "backend_switch");
}

#[test]
fn backend_switch_updates_on_ipc_backends() {
    let mut feature = BackendSwitchFeature::new();
    let (config, error_registry) = make_ctx();
    let ctx = FeatureContext {
        config: &config,
        error_registry: &error_registry,
        ipc_sender: None,
        is_pty_ready: false,
    };

    assert!(feature.backends().is_empty());

    let backends = make_backends();
    feature.on_event(&FeatureEvent::IpcBackends(backends.clone()), &ctx);

    assert_eq!(feature.backends().len(), 2);
    assert_eq!(feature.backends()[0].id, "anthropic");
}

#[test]
fn backend_switch_popup_content_with_backends() {
    let mut feature = BackendSwitchFeature::new();
    let (config, error_registry) = make_ctx();
    let ctx = FeatureContext {
        config: &config,
        error_registry: &error_registry,
        ipc_sender: None,
        is_pty_ready: false,
    };

    feature.on_event(&FeatureEvent::IpcBackends(make_backends()), &ctx);

    let content = feature.popup_content();
    assert!(content.is_some());
    let content = content.unwrap();
    assert_eq!(content.title, "Select Backend");
    assert_eq!(content.lines.len(), 2); // 2 бэкенда
}
```

---

### Phase 4: History Feature Extraction

---

#### Шаг 4.1: Создать `src/features/history.rs` -- обертка над существующим MVI

**Цель**: Обернуть существующий `src/ui/history/` модуль в Feature trait. MVI логика остается на месте, обертка делегирует.

**Файлы для создания**:

**`src/features/history.rs`** (новый файл):

```rust
//! History dialog feature wrapper.
//!
//! Делегирует MVI модулю `src/ui/history/` через Feature trait.

use crate::ui::feature::{
    Feature, FeatureCommand, FeatureContext, FeatureId, KeyCombo, PopupContent,
};
use crate::ui::history::{
    render_history_dialog, HistoryDialogState, HistoryEntry, HistoryIntent, HistoryReducer,
};
use crate::ui::mvi::Reducer;
use ratatui::layout::Rect;
use ratatui::Frame;
use std::sync::Arc;
use term_input::{Direction, KeyInput, KeyKind};

pub struct HistoryFeature {
    state: HistoryDialogState,
    provider: Option<Arc<dyn Fn() -> Vec<HistoryEntry> + Send + Sync>>,
}

impl HistoryFeature {
    pub fn new() -> Self {
        Self {
            state: HistoryDialogState::default(),
            provider: None,
        }
    }

    /// Установить провайдер истории (вызывается из runtime).
    pub fn set_provider(&mut self, provider: Arc<dyn Fn() -> Vec<HistoryEntry> + Send + Sync>) {
        self.provider = Some(provider);
    }

    /// Открыть диалог.
    pub fn open(&mut self) {
        let entries = self
            .provider
            .as_ref()
            .map(|p| p())
            .unwrap_or_default();
        self.dispatch(HistoryIntent::Load { entries });
    }

    /// Закрыть диалог.
    pub fn close(&mut self) {
        self.dispatch(HistoryIntent::Close);
    }

    /// Доступ к состоянию (для рендеринга).
    pub fn state(&self) -> &HistoryDialogState {
        &self.state
    }

    fn dispatch(&mut self, intent: HistoryIntent) {
        self.state = HistoryReducer::reduce(std::mem::take(&mut self.state), intent);
    }
}

impl Feature for HistoryFeature {
    fn id(&self) -> FeatureId {
        "history"
    }

    fn hotkey(&self) -> Option<KeyCombo> {
        Some(KeyCombo::Ctrl('h'))
    }

    fn handle_popup_key(
        &mut self,
        key: &KeyInput,
        _ctx: &FeatureContext<'_>,
    ) -> Option<Vec<FeatureCommand>> {
        match &key.kind {
            KeyKind::Escape | KeyKind::Control('h') => {
                self.close();
                Some(vec![FeatureCommand::ClosePopup])
            }
            KeyKind::Arrow(Direction::Up) => {
                self.dispatch(HistoryIntent::ScrollUp);
                Some(vec![])
            }
            KeyKind::Arrow(Direction::Down) => {
                self.dispatch(HistoryIntent::ScrollDown);
                Some(vec![])
            }
            _ => Some(vec![]),
        }
    }

    fn render_custom(&self, frame: &mut Frame<'_>, _body: Rect) -> bool {
        // Делегируем существующей функции рендеринга
        render_history_dialog(frame, &self.state);
        true
    }
}
```

Обновить `src/features/mod.rs`:

```rust
pub mod backend_switch;
pub mod history;
pub mod status;

use crate::ui::feature::FeatureRegistry;

pub fn create_registry() -> FeatureRegistry {
    let mut registry = FeatureRegistry::new();
    registry.register(Box::new(status::StatusFeature::new()));
    registry.register(Box::new(backend_switch::BackendSwitchFeature::new()));
    registry.register(Box::new(history::HistoryFeature::new()));
    registry
}
```

**Проверка**: `cargo build --features pluggable`

---

### Phase 5: Settings Feature Extraction

---

#### Шаг 5.1: Создать `src/features/settings.rs`

**Цель**: Обернуть Settings MVI в Feature trait. Сложность: `apply_settings()` имеет side effects (PTY restart).

**Файлы для создания**:

**`src/features/settings.rs`** (новый файл):

```rust
//! Settings dialog feature wrapper.

use crate::config::{ClaudeSettingsManager, SettingId};
use crate::ui::app::UiCommand;
use crate::ui::components::PopupDialog;
use crate::ui::feature::{
    Feature, FeatureCommand, FeatureContext, FeatureId, KeyCombo, PopupContent,
};
use crate::ui::settings::{SettingsDialogState, SettingsIntent, SettingsReducer};
use crate::ui::mvi::Reducer;
use crate::ui::theme::{
    ACTIVE_HIGHLIGHT, CLAUDE_ORANGE, HEADER_SEPARATOR, HEADER_TEXT, STATUS_OK,
};
use crate::config::SettingSection;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;
use std::collections::HashMap;
use term_input::{Direction, KeyInput, KeyKind};

pub struct SettingsFeature {
    state: SettingsDialogState,
    settings_manager: ClaudeSettingsManager,
    saved_snapshot: HashMap<SettingId, bool>,
}

impl SettingsFeature {
    pub fn new(manager: ClaudeSettingsManager) -> Self {
        let saved_snapshot = manager.snapshot_values();
        Self {
            state: SettingsDialogState::default(),
            settings_manager: manager,
            saved_snapshot,
        }
    }

    /// Доступ к settings manager.
    pub fn settings_manager(&self) -> &ClaudeSettingsManager {
        &self.settings_manager
    }

    /// Открыть диалог.
    pub fn open(&mut self) {
        let fields = self.settings_manager.to_snapshots();
        self.saved_snapshot = self.settings_manager.snapshot_values();
        self.dispatch(SettingsIntent::Load { fields });
    }

    /// Закрыть диалог (без apply).
    pub fn close(&mut self) {
        self.dispatch(SettingsIntent::Close);
    }

    /// Запросить закрытие (с проверкой dirty).
    pub fn request_close(&mut self) -> bool {
        self.dispatch(SettingsIntent::RequestClose);
        !self.state.is_visible()
    }

    /// Применить настройки. Возвращает FeatureCommand::SendUiCommand(RestartPty) если нужен рестарт.
    pub fn apply(&mut self) -> Vec<FeatureCommand> {
        let fields = match &self.state {
            SettingsDialogState::Visible { fields, .. } => fields.clone(),
            _ => return vec![],
        };

        self.settings_manager.apply_snapshots(&fields);

        if !self.settings_manager.is_dirty(&self.saved_snapshot) {
            self.close();
            return vec![FeatureCommand::ClosePopup];
        }

        let env_vars = self.settings_manager.to_env_vars();
        let cli_args = self.settings_manager.to_cli_args();
        let settings_toml = self.settings_manager.to_toml_map();

        self.saved_snapshot = self.settings_manager.snapshot_values();
        self.close();

        vec![
            FeatureCommand::ClosePopup,
            FeatureCommand::SendUiCommand(UiCommand::RestartPty {
                env_vars,
                cli_args,
                settings_toml,
            }),
        ]
    }

    fn dispatch(&mut self, intent: SettingsIntent) {
        self.state = SettingsReducer::reduce(std::mem::take(&mut self.state), intent);
    }
}

impl Feature for SettingsFeature {
    fn id(&self) -> FeatureId {
        "settings"
    }

    fn hotkey(&self) -> Option<KeyCombo> {
        Some(KeyCombo::Ctrl('e'))
    }

    fn handle_popup_key(
        &mut self,
        key: &KeyInput,
        _ctx: &FeatureContext<'_>,
    ) -> Option<Vec<FeatureCommand>> {
        match &key.kind {
            KeyKind::Escape => {
                let closed = self.request_close();
                if closed {
                    Some(vec![FeatureCommand::ClosePopup])
                } else {
                    Some(vec![])
                }
            }
            KeyKind::Control('e') => {
                self.close();
                Some(vec![FeatureCommand::ClosePopup])
            }
            KeyKind::Arrow(Direction::Up) => {
                self.dispatch(SettingsIntent::MoveUp);
                Some(vec![])
            }
            KeyKind::Arrow(Direction::Down) => {
                self.dispatch(SettingsIntent::MoveDown);
                Some(vec![])
            }
            KeyKind::Char(' ') => {
                self.dispatch(SettingsIntent::Toggle);
                Some(vec![])
            }
            KeyKind::Enter => {
                Some(self.apply())
            }
            _ => Some(vec![]),
        }
    }

    fn render_custom(&self, frame: &mut Frame<'_>, body: Rect) -> bool {
        let SettingsDialogState::Visible {
            fields,
            focused,
            dirty,
            confirm_discard,
        } = &self.state
        else {
            return true;
        };

        let mut lines: Vec<Line> = Vec::new();
        let mut current_section: Option<SettingSection> = None;

        for (idx, field) in fields.iter().enumerate() {
            if current_section != Some(field.section) {
                if current_section.is_some() {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(
                    format!("  \u{2500}\u{2500} {} \u{2500}\u{2500}", field.section.label()),
                    Style::default().fg(CLAUDE_ORANGE),
                )));
                current_section = Some(field.section);
            }

            let is_focused = idx == *focused;
            let checkbox = if field.value { "[x]" } else { "[ ]" };
            let prefix = if is_focused { "  \u{2192} " } else { "    " };

            let base_style = if is_focused {
                Style::default().bg(ACTIVE_HIGHLIGHT)
            } else {
                Style::default()
            };

            let check_color = if field.value { STATUS_OK } else { HEADER_TEXT };

            lines.push(Line::from(vec![
                Span::styled(prefix, base_style.fg(HEADER_TEXT)),
                Span::styled(checkbox, base_style.fg(check_color)),
                Span::styled(format!(" {}", field.label), base_style.fg(HEADER_TEXT)),
            ]));

            lines.push(Line::from(Span::styled(
                format!("      {}", field.description),
                Style::default().fg(HEADER_SEPARATOR),
            )));
        }

        let title = if *dirty { "Settings *" } else { "Settings" };
        let footer = if *confirm_discard {
            "Unsaved changes! Esc: Discard  Enter: Apply"
        } else {
            "Space: Toggle  Enter: Apply  Esc: Cancel"
        };

        PopupDialog::new(title, lines)
            .min_width(50)
            .footer(footer)
            .render(frame, body);

        true
    }
}
```

Обновить `src/features/mod.rs`:

```rust
pub mod backend_switch;
pub mod history;
pub mod settings;
pub mod status;

use crate::config::ClaudeSettingsManager;
use crate::ui::feature::FeatureRegistry;

pub fn create_registry(settings_manager: ClaudeSettingsManager) -> FeatureRegistry {
    let mut registry = FeatureRegistry::new();
    registry.register(Box::new(status::StatusFeature::new()));
    registry.register(Box::new(backend_switch::BackendSwitchFeature::new()));
    registry.register(Box::new(history::HistoryFeature::new()));
    registry.register(Box::new(settings::SettingsFeature::new(settings_manager)));
    registry
}
```

**Проверка**: `cargo build --features pluggable`

---

### Phase 6: Core Refactoring (pluggable path only)

Это ключевая фаза. Здесь мы добавляем `cfg` gates в существующие файлы, создавая два параллельных пути компиляции.

---

#### Шаг 6.1: Расширить App struct (pluggable вариант)

**Цель**: В pluggable пути App хранит `FeatureRegistry` вместо feature-специфичных полей.

**`src/ui/app.rs`**:

Стратегия: используем `cfg_attr` и условную компиляцию для полей и методов.

ДО (строки 1-15):
```rust
use crate::config::{ClaudeSettingsManager, ConfigStore};
use crate::error::ErrorRegistry;
use crate::ipc::{BackendInfo, ProxyStatus};
use crate::metrics::MetricsSnapshot;
use crate::pty::PtyHandle;
use crate::ui::history::{HistoryDialogState, HistoryEntry, HistoryIntent, HistoryReducer};
use crate::ui::mvi::Reducer;
use crate::ui::pty::{PtyIntent, PtyLifecycleState, PtyReducer};
use crate::ui::selection::{GridPos, TextSelection};
use crate::ui::settings::{SettingsDialogState, SettingsIntent, SettingsReducer};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
```

ПОСЛЕ:
```rust
use crate::config::ConfigStore;
use crate::error::ErrorRegistry;
use crate::pty::PtyHandle;
use crate::ui::pty::{PtyIntent, PtyLifecycleState, PtyReducer};
use crate::ui::selection::{GridPos, TextSelection};
use crate::ui::mvi::Reducer;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

// Монолитный путь
#[cfg(not(feature = "pluggable"))]
use crate::config::ClaudeSettingsManager;
#[cfg(not(feature = "pluggable"))]
use crate::ipc::{BackendInfo, ProxyStatus};
#[cfg(not(feature = "pluggable"))]
use crate::metrics::MetricsSnapshot;
#[cfg(not(feature = "pluggable"))]
use crate::ui::history::{HistoryDialogState, HistoryEntry, HistoryIntent, HistoryReducer};
#[cfg(not(feature = "pluggable"))]
use crate::ui::settings::{SettingsDialogState, SettingsIntent, SettingsReducer};

// Pluggable путь
#[cfg(feature = "pluggable")]
use crate::ui::feature::FeatureRegistry;
```

Для `PopupKind` и `Focus`:

ДО (строки 17-29):
```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PopupKind {
    BackendSwitch,
    Status,
    History,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Terminal,
    Popup(PopupKind),
}
```

ПОСЛЕ:
```rust
#[cfg(not(feature = "pluggable"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PopupKind {
    BackendSwitch,
    Status,
    History,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Terminal,
    #[cfg(not(feature = "pluggable"))]
    Popup(PopupKind),
    #[cfg(feature = "pluggable")]
    Popup,  // Попап управляется FeatureRegistry
}
```

Для `App` struct:

ДО (строки 55-89):
```rust
pub struct App {
    should_quit: bool,
    focus: Focus,
    size: Option<(u16, u16)>,
    pub pty_lifecycle: PtyLifecycleState,
    pty_handle: Option<PtyHandle>,
    config: ConfigStore,
    error_registry: ErrorRegistry,
    ipc_sender: Option<UiCommandSender>,
    proxy_status: Option<ProxyStatus>,
    metrics: Option<MetricsSnapshot>,
    backends: Vec<BackendInfo>,
    backend_selection: usize,
    last_ipc_error: Option<String>,
    last_status_refresh: Instant,
    last_metrics_refresh: Instant,
    last_backends_refresh: Instant,
    history_dialog: HistoryDialogState,
    history_provider: Option<Arc<dyn Fn() -> Vec<HistoryEntry> + Send + Sync>>,
    settings_dialog: SettingsDialogState,
    settings_manager: ClaudeSettingsManager,
    settings_saved_snapshot: std::collections::HashMap<crate::config::SettingId, bool>,
    pty_generation: u64,
    selection: Option<TextSelection>,
}
```

ПОСЛЕ:
```rust
pub struct App {
    should_quit: bool,
    focus: Focus,
    size: Option<(u16, u16)>,
    pub pty_lifecycle: PtyLifecycleState,
    pty_handle: Option<PtyHandle>,
    config: ConfigStore,
    error_registry: ErrorRegistry,
    ipc_sender: Option<UiCommandSender>,

    // === Монолитный путь: feature-специфичные поля ===
    #[cfg(not(feature = "pluggable"))]
    proxy_status: Option<ProxyStatus>,
    #[cfg(not(feature = "pluggable"))]
    metrics: Option<MetricsSnapshot>,
    #[cfg(not(feature = "pluggable"))]
    backends: Vec<BackendInfo>,
    #[cfg(not(feature = "pluggable"))]
    backend_selection: usize,
    #[cfg(not(feature = "pluggable"))]
    last_ipc_error: Option<String>,
    #[cfg(not(feature = "pluggable"))]
    last_status_refresh: Instant,
    #[cfg(not(feature = "pluggable"))]
    last_metrics_refresh: Instant,
    #[cfg(not(feature = "pluggable"))]
    last_backends_refresh: Instant,
    #[cfg(not(feature = "pluggable"))]
    history_dialog: HistoryDialogState,
    #[cfg(not(feature = "pluggable"))]
    history_provider: Option<Arc<dyn Fn() -> Vec<HistoryEntry> + Send + Sync>>,
    #[cfg(not(feature = "pluggable"))]
    settings_dialog: SettingsDialogState,
    #[cfg(not(feature = "pluggable"))]
    settings_manager: ClaudeSettingsManager,
    #[cfg(not(feature = "pluggable"))]
    settings_saved_snapshot: std::collections::HashMap<crate::config::SettingId, bool>,

    // === Pluggable путь: FeatureRegistry ===
    #[cfg(feature = "pluggable")]
    pub features: FeatureRegistry,

    // === Общие поля ===
    pty_generation: u64,
    selection: Option<TextSelection>,
}
```

Аналогично `App::new()` и все методы, специфичные для фич, оборачиваются в `#[cfg(not(feature = "pluggable"))]`.

Методы для pluggable пути:

```rust
#[cfg(feature = "pluggable")]
impl App {
    pub fn new_pluggable(config: ConfigStore, features: FeatureRegistry) -> Self {
        Self {
            should_quit: false,
            focus: Focus::Terminal,
            size: None,
            pty_lifecycle: PtyLifecycleState::default(),
            pty_handle: None,
            config,
            error_registry: ErrorRegistry::new(100),
            ipc_sender: None,
            features,
            pty_generation: 0,
            selection: None,
        }
    }

    pub fn show_popup(&self) -> bool {
        matches!(self.focus, Focus::Popup)
    }

    pub fn toggle_feature_popup(&mut self, feature_id: crate::ui::feature::FeatureId) -> bool {
        let opened = self.features.toggle_popup(feature_id);
        self.focus = if opened { Focus::Popup } else { Focus::Terminal };
        opened
    }

    pub fn close_popup(&mut self) {
        self.features.close_popup();
        self.focus = Focus::Terminal;
    }
}
```

**ВАЖНО**: Существующий `App::new()` и все монолитные методы остаются за `#[cfg(not(feature = "pluggable"))]`. Новый `App::new_pluggable()` за `#[cfg(feature = "pluggable")]`.

**Проверка**:
```bash
cargo build                      # OK (монолитный путь, как раньше)
cargo build --features pluggable # OK (pluggable путь)
cargo test                       # OK (тесты используют монолитный путь)
```

**Откат**: `git checkout src/ui/app.rs`

---

#### Шаг 6.2: Создать pluggable draw() вариант

**`src/ui/render.rs`**:

ДО (строки 1-9, imports):
```rust
use crate::config::SettingSection;
use crate::error::ErrorSeverity;
use crate::ui::app::{App, PopupKind};
use crate::ui::components::PopupDialog;
use crate::ui::footer::Footer;
use crate::ui::header::Header;
use crate::ui::history::render_history_dialog;
use crate::ui::layout::layout_regions;
use crate::ui::settings::SettingsDialogState;
use crate::ui::terminal::TerminalBody;
use crate::ui::theme::{...};
```

ПОСЛЕ:
```rust
use crate::ui::app::App;
use crate::ui::footer::Footer;
use crate::ui::header::Header;
use crate::ui::layout::layout_regions;
use crate::ui::terminal::TerminalBody;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;
use ratatui::Frame;
use std::sync::Arc;

#[cfg(not(feature = "pluggable"))]
use crate::config::SettingSection;
#[cfg(not(feature = "pluggable"))]
use crate::error::ErrorSeverity;
#[cfg(not(feature = "pluggable"))]
use crate::ui::app::PopupKind;
#[cfg(not(feature = "pluggable"))]
use crate::ui::components::PopupDialog;
#[cfg(not(feature = "pluggable"))]
use crate::ui::history::render_history_dialog;
#[cfg(not(feature = "pluggable"))]
use crate::ui::settings::SettingsDialogState;
#[cfg(not(feature = "pluggable"))]
use crate::ui::theme::{
    ACTIVE_HIGHLIGHT, CLAUDE_ORANGE, HEADER_SEPARATOR, HEADER_TEXT, STATUS_ERROR, STATUS_OK,
    STATUS_WARNING,
};
#[cfg(not(feature = "pluggable"))]
use std::time::{Duration, SystemTime};
```

Текущая `draw()` оборачивается в `#[cfg(not(feature = "pluggable"))]`.

Новая pluggable `draw()`:

```rust
#[cfg(feature = "pluggable")]
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let (header, body, footer) = layout_regions(area);

    // Header -- нужен proxy_status из StatusFeature
    // Получаем через downcast
    let proxy_status = app
        .features
        .get("status")
        .and_then(|f| {
            // Нужен downcast к StatusFeature
            // Решение: добавить метод proxy_status_for_header() в registry
            None::<&crate::ipc::ProxyStatus> // placeholder
        });

    let header_widget = Header::new();
    frame.render_widget(
        header_widget.widget(proxy_status, app.error_registry()),
        header,
    );
    frame.render_widget(Clear, body);
    if let Some(emu) = app.emulator() {
        frame.render_widget(TerminalBody::new(Arc::clone(&emu), app.selection()), body);
        if app.is_pty_ready() && app.focus_is_terminal() && app.scrollback() == 0
            && body.width > 0 && body.height > 0
        {
            let cursor = emu.lock().cursor();
            if cursor.visible {
                let x = body.x + cursor.col.min(body.width.saturating_sub(1));
                let y = body.y + cursor.row.min(body.height.saturating_sub(1));
                frame.set_cursor_position((x, y));
            }
        }
    }

    let footer_widget = Footer::new();
    frame.render_widget(footer_widget.widget(footer), footer);

    // Попапы рендерятся через registry
    app.features.render_active_popup(frame, body);
}
```

**Проблема downcast**: `Feature` -- trait object, нет `Any` downcast. Решение: добавить метод `as_any()` в Feature trait или вспомогательный метод в registry.

Лучшее решение: добавить в `FeatureRegistry`:

```rust
/// Получить proxy_status для Header (из StatusFeature).
/// Специальный accessor для cross-feature данных.
pub fn proxy_status(&self) -> Option<&ProxyStatus> {
    for feature in &self.features {
        if feature.id() == "status" {
            // Используем Any для downcast
            // ...
        }
    }
    None
}
```

Но это требует `Any`. Альтернатива: StatusFeature хранится отдельно, не в registry.

**Принятое решение**: Добавить к `Feature` trait метод:

```rust
/// Downcast к Any для cross-feature данных.
fn as_any(&self) -> &dyn std::any::Any;
```

С default impl (паника) и override в StatusFeature. Тогда в registry:

```rust
pub fn proxy_status(&self) -> Option<&ProxyStatus> {
    self.get("status")
        .and_then(|f| f.as_any().downcast_ref::<crate::features::status::StatusFeature>())
        .and_then(|s| s.proxy_status())
}
```

Это работает, но создает tight coupling. Поэтому лучший подход:

**Финальное решение**: `proxy_status` остается в App (shared infrastructure), не переносится в StatusFeature. StatusFeature получает его через `FeatureEvent::IpcStatus` и хранит копию для попапа, но App тоже хранит для Header.

Это означает, что в pluggable App:

```rust
pub struct App {
    // ... core fields ...
    #[cfg(feature = "pluggable")]
    pub features: FeatureRegistry,
    #[cfg(feature = "pluggable")]
    proxy_status: Option<ProxyStatus>,  // Shared для Header
    // ...
}
```

Тогда pluggable `draw()` работает как раньше с `app.proxy_status()`.

---

#### Шаг 6.3: Создать pluggable classify_key() вариант

**`src/ui/input.rs`**:

Текущая `classify_key()` оборачивается в `#[cfg(not(feature = "pluggable"))]`.

Новая pluggable версия:

```rust
#[cfg(feature = "pluggable")]
pub fn classify_key(app: &mut App, key: &KeyInput) -> InputAction {
    use crate::ui::feature::{FeatureCommand, FeatureContext};

    // Global hotkeys
    match &key.kind {
        KeyKind::Control('q') => {
            app.request_quit();
            return InputAction::None;
        }
        KeyKind::Control('v') => {
            return InputAction::Forward;
        }
        KeyKind::Control('r') => {
            app.request_restart_claude();
            return InputAction::None;
        }
        _ => {}
    }

    // Popup active -- делегируем фиче
    if app.show_popup() {
        let ctx = FeatureContext {
            config: &app.config,
            error_registry: &app.error_registry,
            ipc_sender: app.ipc_sender.as_ref(),
            is_pty_ready: app.is_pty_ready(),
        };
        if let Some(commands) = app.features.handle_popup_key(key, &ctx) {
            let external = app.features.execute_commands(commands);
            for cmd in external {
                match cmd {
                    FeatureCommand::SendUiCommand(ui_cmd) => {
                        app.send_command(ui_cmd);
                    }
                    FeatureCommand::RequestQuit => {
                        app.request_quit();
                    }
                    _ => {}
                }
            }
            // Если попап закрылся, обновить focus
            if !app.features.has_active_popup() {
                app.focus = Focus::Terminal;
            }
            return InputAction::None;
        }
        return InputAction::None;
    }

    // Feature hotkeys
    if let Some(feature_id) = app.features.match_hotkey(key) {
        let opened = app.toggle_feature_popup(feature_id);
        if opened {
            // Trigger refresh при открытии
            // StatusFeature: request_status_refresh + metrics
            // BackendSwitch: request_backends_refresh
            // Это обрабатывается через on_tick или init
        }
        return InputAction::None;
    }

    InputAction::Forward
}
```

Аналогично оборачиваем `handle_popup_key()` и все sub-handlers.

---

#### Шаг 6.4: Создать pluggable Tick handler

В `runtime.rs`, блок `Ok(AppEvent::Tick)`:

```rust
#[cfg(not(feature = "pluggable"))]
{
    app.on_tick();
    if app.should_refresh_status(STATUS_REFRESH_INTERVAL) {
        app.request_status_refresh();
    }
    if app.popup_kind() == Some(crate::ui::app::PopupKind::BackendSwitch)
        && app.should_refresh_backends(BACKENDS_REFRESH_INTERVAL)
    {
        app.request_backends_refresh();
    }
}

#[cfg(feature = "pluggable")]
{
    let ctx = crate::ui::feature::FeatureContext {
        config: &app.config,
        error_registry: &app.error_registry,
        ipc_sender: app.ipc_sender.as_ref(),
        is_pty_ready: app.is_pty_ready(),
    };
    let commands = app.features.tick(&ctx);
    let external = app.features.execute_commands(commands);
    for cmd in external {
        match cmd {
            crate::ui::feature::FeatureCommand::SendUiCommand(ui_cmd) => {
                app.send_command(ui_cmd);
            }
            crate::ui::feature::FeatureCommand::RequestQuit => {
                app.request_quit();
            }
            _ => {}
        }
    }
}
```

---

#### Шаг 6.5: Создать pluggable IPC event handlers

В `runtime.rs`, блоки IpcStatus/IpcBackends/IpcError:

```rust
Ok(AppEvent::IpcStatus(status)) => {
    #[cfg(not(feature = "pluggable"))]
    app.update_status(status);

    #[cfg(feature = "pluggable")]
    {
        app.proxy_status = Some(status.clone());
        let ctx = /* FeatureContext */;
        app.features.broadcast_event(
            &crate::ui::feature::FeatureEvent::IpcStatus(status),
            &ctx,
        );
    }
}
```

Аналогично для IpcBackends, IpcError.

---

### Phase 7: Footer & Header Integration

---

#### Шаг 7.1: Footer принимает динамические спаны от registry

**`src/ui/footer.rs`**:

Добавить вариант `widget()` для pluggable:

```rust
#[cfg(feature = "pluggable")]
pub fn widget_with_feature_spans(
    &self,
    area: Rect,
    extra_spans: Vec<Span<'static>>,
) -> Paragraph<'static> {
    // Добавить extra_spans между hints и version
    // ...
}
```

---

#### Шаг 7.2: Header использует proxy_status из App

Уже решено в шаге 6.2: `proxy_status` остается в App как shared infrastructure. Header не меняется.

---

### Phase 8: Claude Code Version Monitor (целевая фича)

---

#### Шаг 8.1: Создать `src/features/version_monitor.rs`

**Цель**: Демонстрация мощи pluggable архитектуры -- новая фича за 1 файл + 1 строку регистрации.

```rust
//! Claude Code Version Monitor feature.
//!
//! Периодически проверяет версию Claude Code и показывает её в footer.

use crate::ui::feature::{
    Feature, FeatureCommand, FeatureContext, FeatureId, PopupContent,
};
use crate::ui::theme::HEADER_TEXT;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use std::process::Command;
use std::time::{Duration, Instant};

const VERSION_CHECK_INTERVAL: Duration = Duration::from_secs(300); // 5 min

pub struct VersionMonitorFeature {
    claude_version: Option<String>,
    last_check: Instant,
    checked_once: bool,
}

impl VersionMonitorFeature {
    pub fn new() -> Self {
        Self {
            claude_version: None,
            last_check: Instant::now(),
            checked_once: false,
        }
    }

    fn check_version(&mut self) {
        // claude --version выводит что-то вроде "claude 1.0.3"
        let output = Command::new("claude")
            .arg("--version")
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let version = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .to_string();
                self.claude_version = Some(version);
            }
        }
        self.last_check = Instant::now();
        self.checked_once = true;
    }
}

impl Feature for VersionMonitorFeature {
    fn id(&self) -> FeatureId {
        "version_monitor"
    }

    fn init(&mut self, _ctx: &FeatureContext<'_>) {
        self.check_version();
    }

    fn on_tick(&mut self, _ctx: &FeatureContext<'_>) -> Vec<FeatureCommand> {
        if self.last_check.elapsed() >= VERSION_CHECK_INTERVAL {
            self.check_version();
        }
        vec![]
    }

    fn footer_spans(&self) -> Vec<Span<'static>> {
        if let Some(version) = &self.claude_version {
            let text_style = Style::default()
                .fg(HEADER_TEXT)
                .add_modifier(Modifier::DIM);
            vec![Span::styled(
                format!(" CC: {} ", version),
                text_style,
            )]
        } else {
            vec![]
        }
    }
}
```

Регистрация в `src/features/mod.rs`:

```rust
pub mod version_monitor;

// В create_registry():
registry.register(Box::new(version_monitor::VersionMonitorFeature::new()));
```

**Вот и всё.** Ни одного изменения в app.rs, render.rs, input.rs, runtime.rs.

---

### Phase 9: Cleanup & Finalization

---

#### Шаг 9.1: CI setup -- тестировать оба feature flag состояния

В `.github/workflows/ci.yml` (или эквивалент):

```yaml
jobs:
  test:
    strategy:
      matrix:
        features: ["", "--features pluggable"]
    steps:
      - run: cargo build ${{ matrix.features }}
      - run: cargo test ${{ matrix.features }}
      - run: cargo clippy ${{ matrix.features }} -- -D warnings
```

---

#### Шаг 9.2: Удалить монолитный путь (когда pluggable стабилен)

1. Удалить все `#[cfg(not(feature = "pluggable"))]` блоки
2. Удалить `PopupKind` enum
3. Удалить feature-специфичные поля из App
4. Удалить `handle_popup_key()`, `handle_backend_switch_key()` и т.д. из input.rs
5. Удалить inline рендеринг из render.rs

---

#### Шаг 9.3: Убрать feature flag (pluggable = default)

В `Cargo.toml`:

```toml
[features]
default = ["pluggable"]
pluggable = []
```

Позже: убрать все `#[cfg(feature = "pluggable")]` -- код становится единственным путем.

---

## 5. Матрица рисков

| Шаг | Риск | Вероятность | Влияние | Митигация |
|-----|------|-------------|---------|-----------|
| 1.2 | `dead_code` lint на типы за cfg | Средняя | Блокер | `#[allow(dead_code)]` временно, убрать в 2.1 |
| 2.1 | StatusFeature дублирует данные с App | Низкая | Низкое | Ожидаемо в Strangler Fig, удалится в Phase 6 |
| 6.1 | cfg на полях struct сложен для поддержки | Средняя | Среднее | Минимизировать кол-во cfg; тестировать оба пути |
| 6.2 | proxy_status нужен и App и StatusFeature | Высокая | Среднее | Оставить в App как shared infrastructure |
| 6.3 | Дублирование логики в двух classify_key | Средняя | Среднее | Phase 9 удалит дублирование |
| 5.1 | apply_settings side effects (PTY restart) | Средняя | Высокое | FeatureCommand::SendUiCommand(RestartPty) |
| 9.2 | Удаление монолитного пути может сломать | Низкая | Высокое | Делать только когда pluggable полностью протестирован |
| Все | `unused_imports` при cfg switching | Высокая | Блокер | Группировать imports за cfg |

---

## 6. Стратегия тестирования

### Принцип: каждый шаг проверяется двумя командами

```bash
cargo build                      # Монолитный путь
cargo build --features pluggable # Pluggable путь
```

### Тесты по фазам

| Фаза | Тест | Команда |
|------|------|---------|
| Phase 1 | Оба пути компилируются | `cargo build && cargo build --features pluggable` |
| Phase 2 | StatusFeature unit tests | `cargo test --features pluggable status_feature` |
| Phase 2 | Существующие тесты не сломаны | `cargo test` |
| Phase 3 | BackendSwitchFeature tests | `cargo test --features pluggable backend_switch` |
| Phase 4 | HistoryFeature wrapper | `cargo test --features pluggable` |
| Phase 5 | SettingsFeature apply | `cargo test --features pluggable` |
| Phase 6 | Полный pluggable путь работает | `cargo run --features pluggable` (ручной) |
| Phase 6 | Все существующие тесты проходят | `cargo test` |
| Phase 8 | VersionMonitor footer | `cargo run --features pluggable` (ручной) |
| Phase 9 | CI matrix | GitHub Actions |

### Интеграционные тесты (все в `tests/`)

```
tests/status_feature.rs          -- #![cfg(feature = "pluggable")]
tests/backend_switch_feature.rs  -- #![cfg(feature = "pluggable")]
tests/feature_registry.rs        -- #![cfg(feature = "pluggable")]
tests/pluggable_app.rs           -- #![cfg(feature = "pluggable")]
```

Существующие тесты (`tests/app_lifecycle.rs`, `tests/settings_reducer.rs` и т.д.) продолжают работать без `--features pluggable` -- они используют монолитный путь.

---

## 7. Приложение: Целевое дерево файлов

```
src/
  lib.rs                          # + #[cfg(feature = "pluggable")] pub mod features;
  features/                       # НОВАЯ директория (за cfg)
    mod.rs                        # create_registry()
    status.rs                     # StatusFeature
    backend_switch.rs             # BackendSwitchFeature
    history.rs                    # HistoryFeature (обертка над ui/history/)
    settings.rs                   # SettingsFeature (обертка над ui/settings/)
    version_monitor.rs            # VersionMonitorFeature (новая фича)
  ui/
    mod.rs                        # + #[cfg(feature = "pluggable")] pub mod feature;
    feature.rs                    # НОВЫЙ: Feature trait, FeatureRegistry, FeatureContext
    app.rs                        # МОДИФИЦИРОВАН: cfg-gated fields + new_pluggable()
    render.rs                     # МОДИФИЦИРОВАН: cfg-gated draw()
    input.rs                      # МОДИФИЦИРОВАН: cfg-gated classify_key()
    runtime.rs                    # МОДИФИЦИРОВАН: cfg-gated tick/events
    events.rs                     # БЕЗ ИЗМЕНЕНИЙ
    footer.rs                     # МОДИФИЦИРОВАН: widget_with_feature_spans()
    header.rs                     # БЕЗ ИЗМЕНЕНИЙ
    layout.rs                     # БЕЗ ИЗМЕНЕНИЙ
    components/
      mod.rs                      # БЕЗ ИЗМЕНЕНИЙ
      popup.rs                    # БЕЗ ИЗМЕНЕНИЙ
    mvi/                          # БЕЗ ИЗМЕНЕНИЙ
      mod.rs
      state.rs
      reducer.rs
      intent.rs
    history/                      # БЕЗ ИЗМЕНЕНИЙ (используется оберткой)
      mod.rs
      state.rs
      intent.rs
      reducer.rs
      dialog.rs
    settings/                     # БЕЗ ИЗМЕНЕНИЙ (используется оберткой)
      mod.rs
      state.rs
      intent.rs
      reducer.rs
    pty/                          # БЕЗ ИЗМЕНЕНИЙ
      mod.rs
      state.rs
      intent.rs
      reducer.rs
    selection.rs                  # БЕЗ ИЗМЕНЕНИЙ
    terminal.rs                   # БЕЗ ИЗМЕНЕНИЙ
    terminal_guard.rs             # БЕЗ ИЗМЕНЕНИЙ
    theme.rs                      # БЕЗ ИЗМЕНЕНИЙ

tests/
  status_feature.rs               # НОВЫЙ (cfg pluggable)
  backend_switch_feature.rs       # НОВЫЙ (cfg pluggable)
  feature_registry.rs             # НОВЫЙ (cfg pluggable)
  pluggable_app.rs                # НОВЫЙ (cfg pluggable)
  # ... все существующие тесты без изменений ...

Cargo.toml                        # МОДИФИЦИРОВАН: pluggable feature flag
```

### Количество изменений по файлам

| Действие | Файлы |
|----------|-------|
| Новые файлы | 8 (`feature.rs`, `features/` 6 файлов, тесты 4 файла) |
| Модифицированные файлы | 7 (`Cargo.toml`, `lib.rs`, `ui/mod.rs`, `app.rs`, `render.rs`, `input.rs`, `runtime.rs`, `footer.rs`) |
| Без изменений | ~40+ файлов |

### Порядок выполнения для параллельной разработки

После Phase 1 (Foundation), следующие фазы могут выполняться параллельно:

```
Phase 1 (Foundation)
    |
    +-- Phase 2 (Status)
    +-- Phase 3 (BackendSwitch)
    +-- Phase 4 (History)
    +-- Phase 5 (Settings)
    |
    v
Phase 6 (Core Refactoring) -- зависит от 2-5
    |
    v
Phase 7 (Footer/Header) -- зависит от 6
    |
    v
Phase 8 (Version Monitor) -- зависит от 6
    |
    v
Phase 9 (Cleanup) -- зависит от все
```
