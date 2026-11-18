# Нижняя панель для долгих процессов («Hide running terminals») в Warpium

Исследование и дизайн от 2026-11-18, пункт 14 роадмапа. Ветка `master` (база `4ad707430`); зависимость, пункт 12, читалась по `origin/claude/tab-project-root` (PR #25, его не трогали). Продуктовый код не писался, спайков не было: все выводы сделаны по чтению кода, ссылки даны как `файл:строка` на базе `4ad707430`.

---

## 1. TL;DR

1. **Отцепить терминальную панель от раскладки, не убивая PTY, можно, и для этого почти всё уже есть.** PTY живёт столько же, сколько модель `TerminalManager` (`Drop` → `shutdown_event_loop`, `app/src/terminal/local_tty/terminal_manager.rs:293`). Эту модель держит `PaneView` терминала (`app/src/pane_group/pane/terminal_pane.rs:97-101`, `:194-199`), а его держит `Box<dyn AnyPaneContent>` в `PaneGroup::pane_contents` (`app/src/pane_group/mod.rs:888`). Дерево раскладки `PaneData` хранит только `PaneId` (`app/src/pane_group/tree.rs:49-53`). Пока содержимое лежит в `pane_contents`, процесс жив, даже если панели нет в дереве.
2. **Прецедент «вне дерева» уже работает в upstream:** панели дочерних агентов создаются в `pane_contents` и не попадают в дерево (`attach_child_pane_off_tree`, `mod.rs:4225-4245`; `insert_terminal_pane_hidden_for_child_agent`, `mod.rs:4014-4056`). Потом их вставляют в раскладку обычным `panes.split` (`mod.rs:7453-7458`) без повторного `attach`. Есть и скрытие на месте: `HiddenPaneReason::FromJob`. Через него локальная сессия загрузки файла по SSH работает невидимой (`terminal_pane.rs:1283-1291`, `mod.rs:5372-5404`). Нижняя панель — третий потребитель того же механизма.
3. **Перенос между вкладками переиспользуется как есть.** `PaneGroup::remove_pane_for_move` (`mod.rs:4399-4446`) вызывает `detach(DetachType::Moved)`. Он намеренно не чистит блоки, AI-разговоры и CLI-agent-сессию (`terminal_pane.rs:393-445`), а содержимое `Box` возвращает вызывающему. Workspace кладёт его в другую вкладку через `add_tab_from_existing_pane` / `add_pane_as_hidden` (`app/src/workspace/view.rs:17192`, `:17409-17414`). Для «в панель другой вкладки» нужен только новый приёмник: «положить в док вне дерева».
4. **Цена скрытого терминала такая же, как у терминала в неактивной вкладке.** Байты PTY разбирает фоновый поток event loop (`crates/warp_terminal/src/local_tty/event_loop.rs:201-271`, `:333`) прямо в `TerminalModel`, независимо от UI. Пробуждения UI троттлятся до 60/с (`app/src/terminal/view.rs:626-628`, `:3810-3813`). Каждое из них делает `ctx.notify()` (`view.rs:10237-10281`): это инвалидация окна, `render_view` скрытого вида (без layout и paint) и перерисовка кадра (`crates/warpui_core/src/core/app.rs:4112-4118`, `:3512-3526`; `presenter.rs:318-331`). Отдельного троттлинга для невидимых видов нет. Это не регрессия: так же сегодня живёт `logcat` или `gradle` в фоновой вкладке. Память ограничена `maximum_grid_size` (`app/src/terminal/terminal_manager.rs:120-124`).
5. **Куда встраивать.** Док должен жить **внутри `PaneGroup`**: вкладка и есть `PaneGroup`, а решение владельца «панель на вкладку/проект» так выполняется само. Точка вставки — одна строка в `PaneGroup::render`, сразу после дерева (`mod.rs:8374`): `column.add_child(Shrinkable::new(1., main_content))`. Боковые панели устроены иначе: это view на уровне workspace, а открытость и ширина хранятся на вкладке (`PaneGroup::left_panel_open/right_panel_open`, `mod.rs:929-934`; рендер в `workspace/view.rs:22650-22690`, `:23556-23597`). Для дока, который **владеет панелями** вкладки, этот путь хуже: чужие `pane_contents` пришлось бы рендерить из workspace.
6. **Главные риски, найденные в коде:**
   - закрытие последней видимой панели закрывает всю вкладку (`mod.rs:4871-4881`), а с ней и процессы в доке;
   - `exit` в терминале дока идёт в `close_pane` (`terminal_pane.rs:979-993`), и без отдельной ветки при одной видимой панели в дереве это закроет вкладку;
   - split и навигация от сфокусированной панели, которой нет в дереве (`panes.split` вернёт `false`);
   - сохранение сессии: скрытые панели в снапшот не попадают вообще (`mod.rs:2147-2154`);
   - Logcat — одна панель на **окно** (`LogcatPaneManager::find_pane(window_id)`, `workspace/view/warpium_android.rs:43`), а док — на вкладку.
7. **План (6 PR):**
   - (1) модель дока в `PaneGroup` и «Move to panel / Restore to layout» за флагом, без UI-полировки;
   - (2) UI дока: подвкладки, resize, свёрнутая полоса чипов со статусом;
   - (3) персистентность и восстановление сессии;
   - (4) `target = "panel"` в `.warpium/toolbar.toml`;
   - (5) Logcat и Android Run в доке;
   - (6) перенос между вкладками, drag & drop, раскатка флага.

   Каждый шаг независимо полезен и тестируем.

---

## 2. Вопрос 1: можно ли отцепить терминал от раскладки, не теряя PTY

### 2.1. Кто чем владеет

```
Workspace.tabs[i].pane_group : ViewHandle<PaneGroup>           (workspace/view.rs)
└─ PaneGroup
   ├─ panes: PaneData            — дерево PaneNode::{Leaf(PaneId), Branch}   (mod.rs:883, tree.rs:49)
   │   └─ hidden_panes: Vec<HiddenPane{pane_id, reason}>                     (tree.rs:52-75)
   ├─ focus_state: ModelHandle<PaneGroupFocusState>                          (mod.rs:885)
   └─ pane_contents: HashMap<PaneId, Box<dyn AnyPaneContent>>                (mod.rs:888)
       └─ TerminalPane { …, view: ViewHandle<PaneView<TerminalView>> }       (terminal_pane.rs:89-101)
           └─ PaneStack: Vec<(ModelHandle<Box<dyn TerminalManager>>, ViewHandle<TerminalView>)>
               └─ TerminalManager — event loop + PTY; Drop ⇒ shutdown        (local_tty/terminal_manager.rs:293)
```

Комментарий в `TerminalPane` прямо фиксирует порядок дропа: `terminal_manager` уничтожается раньше `view`, чтобы остановить event loop без дедлоков (`terminal_pane.rs:97-101`). Значит, **PTY умирает ровно тогда, когда из `pane_contents` удаляется `Box`**. Удаление из дерева (`PaneData::remove`, `tree.rs:470-478`) процесс не трогает.

Важная деталь: `PaneGroup::pane_ids()` перечисляет `pane_contents`, а не дерево (`mod.rs:4287-4289`). Поэтому всё, что идёт по `pane_contents`, увидит и панели из дока, в том числе:

- предупреждение о запущенных процессах при закрытии и выходе (`quit_warning/mod.rs:82-96` → `pane_sessions`, `mod.rs:2554-2562`);
- навигационная палитра «running».

Это то, что нужно. Код, которому нужны только видимые панели, уже ходит через `visible_pane_ids()` / `visible_terminal_views()` (`tree.rs:486-494`, `mod.rs:7968-7975`). Upstream специально так разделил их из-за дочерних агентов (комментарий в `mod.rs:7671-7676`).

### 2.2. Три существующих режима «панель есть, но её не видно»

| Механизм | Где | Дерево | `attach` | В снапшоте | Для чего |
|---|---|---|---|---|---|
| `HiddenPaneReason::FromJob` | `tree.rs:289-301`, `mod.rs:5372-5404` | остаётся листом, не рендерится | один раз | нет | невидимая локальная сессия загрузки по SSH (`terminal_pane.rs:1283-1291`) |
| `HiddenPaneReason::Closed` (undo close) | `tree.rs:337-365`, `mod.rs:4893-4904` | остаётся листом | `detach(HiddenForClose)`, при undo снова `attach` | нет | восстановление закрытой панели |
| Вне дерева (дочерние агенты) | `mod.rs:4014-4056`, `:4225-4245`, `:7453-7458` | нет | один раз при создании | нет (пересоздаётся) | панели дочерних агентов, появляются по клику на «пилюлю» |

Для дока есть два варианта.

**А. Новый `HiddenPaneReason::Docked`: лист остаётся в дереве.**

- Плюсы: на восстановлении в раскладку панель вернётся точно на прежнее место (flex и соседи сохраняются). Все помощники вида `visible_*` и `has_horizontal_split` (`tree.rs:633-655`) уже учитывают скрытые панели.
- Минусы:
  - нужно править `render` ветвей (`tree.rs:1056-1100`): там отдельные проверки для `FromMove`;
  - `snapshot_for_node` выбрасывает любые скрытые листья (`mod.rs:2147-2154`);
  - `move_pane_with_root_split` и `clear_hidden_*` трогают `hidden_panes` (`mod.rs:5680-5700`);
  - если все видимые панели закрыты, а лист дока остался, дерево состоит из одних скрытых листьев.

**Б. Вне дерева, как дочерние агенты (рекомендуется).** Панель убирается из `PaneData` (`panes.remove`), остаётся в `pane_contents` и записывается в новое поле `PaneGroup::warpium_dock: BottomDock`. Восстановление в раскладку повторяет путь «split-off» дочернего агента: `panes.split(base, id, dir)` или `split_root` (`mod.rs:7453-7458`), затем `handle_pane_count_change` и `focus_pane`.

- Плюсы: дерево ничего не знает о доке, снапшот дерева корректен без правок, `attach` и `detach` вообще не вызываются, то есть подписки `PaneGroup` на вид (`terminal_pane.rs:268-391`) остаются живыми.
- Минус: «прежнее место» не помнится. Для JetBrains-модели это нормально: «Move to editor» там тоже открывает рядом с текущим. Подсказку можно хранить как `restore_hint: Option<(PaneId сосед, Direction)>`, это дёшево.

Отдельный `detach` для «Move to panel» **не нужен**: панель остаётся в той же `PaneGroup`, подписки (`ctx.subscribe_to_view(&self.view, …)`, `terminal_pane.rs:285-287`) продолжают работать. Например, `Event::Exited` от шелла дойдёт до группы.

### 2.3. Close vs move-to-tab vs hide

| Операция | Код | Что с PTY |
|---|---|---|
| Закрыть панель (с undo) | `close_pane` → `detach(HiddenForClose)` + `hide_closed_pane` (`mod.rs:4807-4904`) → по истечении grace-периода `cleanup_closed_pane` → `detach(Closed)` + `pane_contents.remove` (`mod.rs:5629-5652`) | живёт до конца grace-периода, потом дроп |
| Закрыть без undo | `discard_pane` → `cleanup_closed_pane` (`mod.rs:4570-4597`) | дроп сразу |
| Перенести в другую вкладку | `remove_pane_for_move` → `detach(Moved)` → `Box` наружу (`mod.rs:4399-4446`); приёмник `add_tab_from_existing_pane` (`workspace/view.rs:13301`) или `add_pane_as_hidden` (`mod.rs:5331-5350`, `workspace/view.rs:17409-17414`) | жив; `detach(Moved)` не удаляет блоки и CLI-agent-сессию (`terminal_pane.rs:399-445`) |
| Закрыть вкладку | `detach_panes(HiddenForClose)` по **всем** `pane_contents` (`mod.rs:7836-7843`), затем undo-стек вкладки | панели дока умирают вместе с вкладкой. Это совпадает с решением «панель на вкладку» |
| **Move to panel (новое)** | `panes.remove(id)` + `warpium_dock.push(id)` без `detach` | жив, продолжает писать в грид |

Переносить панель из дока в док другой вкладки нужно так: `remove_pane_for_move`, затем новый `PaneGroup::adopt_into_dock(Box)`. Это аналог `attach_child_pane_off_tree`: вставить в `pane_contents`, вызвать `try_attach_pane`, не добавляя в дерево. Повторный `attach` после `detach(Moved)` — штатный сценарий, так работает перенос вкладок.

### 2.4. Ловушки, которые надо закрыть в PR 1

1. **Последняя видимая панель.** `close_pane` при `visible_pane_count() == 1` шлёт `Event::Exited` и закрывает вкладку (`mod.rs:4871-4881`; без undo-флага `pane_count() == 1`, `mod.rs:4911-4921`). Есть и интеграционный тест на это поведение, `test_tab_closes_when_last_visible_pane_closed` (`crates/integration/src/test/pane_restoration.rs:388`). При непустом доке нужна другая политика. Рекомендация: открыть в раскладке новый терминал в корне проекта, как JetBrains оставляет tool window открытым после закрытия всех редакторов. Альтернатива — промоутить активную вкладку дока в раскладку.
2. **`exit` в шелле панели из дока.** `Event::Exited` → `group.close_pane(pane_id)` (`terminal_pane.rs:979-993`). Для панели вне дерева ветка с `visible_pane_count() == 1` ошибочно закроет вкладку, а `hide_closed_pane` положит в `hidden_panes` id, которого нет в дереве. Нужна ранняя ветка `if self.warpium_dock.contains(pane_id)` по образцу ветки дочерних агентов (`mod.rs:4823-4851`). Она убирает панель из дока и делает `discard_pane`.
3. **Фокус на панели вне дерева.** `focus_state.focused_pane_id` может указывать на панель дока. Тогда `PaneGroupAction::Add(Direction)`, resize и навигация стрелками (`mod.rs:440-480`) должны брать последнюю сфокусированную панель **дерева** (а split в доке — отдельная фича позже). Иначе `panes.split` вернёт `false`.
4. **Навигация «перейти к сессии».** `reveal_and_focus_pane` сейчас только пишет warning для панели вне дерева (`mod.rs:5007-5030`). Нужно, чтобы панель из дока раскрывала док и выбирала её подвкладку.
5. **Maximize.** `render` при `is_focused_pane_maximized` рендерит только сфокусированную панель (`mod.rs:8360-8362`). Если она в доке, это должно означать «развернуть док на всю вкладку».

### 2.5. Цена «скрыт, но жив»

- **Разбор вывода.** Поток event loop (`event_loop.rs:333-346`) читает PTY буфером 256 КБ (`READ_BUFFER_SIZE`, `:26`) и под `FairMutex` пишет в `TerminalModel` порциями до 64 КБ (`MAX_LOCKED_READ`, `:30`, `:244-271`). UI в этом не участвует, видимость панели ни на что не влияет. Грид работает всегда, иначе потерялись бы вывод и состояние alt-screen.
- **Пробуждение UI.** `throttle(WAKEUP_THROTTLE_PERIOD)`, 60/с (`view.rs:626-628`, `:3810-3813`). В `handle_terminal_wakeup` (`view.rs:10237-10281`): перезапуск поиска (если открыт), перефильтровка активного блока (`maybe_refilter_active_block_output`), пересчёт высоты блока, затем `ctx.notify()`.
- **Цена `notify`.** Вид попадает в `window_invalidations.updated` (`app.rs:4112-4118`). Затем `update_windows` дёргает перерисовку окна (`app.rs:3512-3526`), а `Presenter::invalidate` вызывает `render_view` этого вида (`presenter.rs:318-331`). Элемент строится, но в сцену не попадает, потому что его нет в дереве `ChildView`. Остаётся полный кадр окна до 60 раз в секунду.
- **Вывод.** Скрытый терминал с болтливым процессом (`logcat` в шелле, `gradle --info`) стоит столько же, сколько такой же терминал в фоновой вкладке. Отдельный троттлинг не обязателен для MVP. Если профилирование покажет проблему (VM-проверка в фазе 2: 5 спрятанных `yes | head -c 1G`-подобных потоков), есть дешёвое улучшение: в доке не делать `ctx.notify()` для невидимой подвкладки. Это одна проверка в `handle_terminal_wakeup`, а значит правка upstream-файла, её лучше отложить.
- **Размер PTY.** Пока панель свёрнута, PTY держит последний размер. Когда подвкладка показывается в доке другой ширины, срабатывает обычный путь resize после layout (`view.rs:3820`, `after_terminal_view_layout`), и полноэкранные TUI получают SIGWINCH.

---

## 3. Вопрос 2: где пристыковать док и как устроены существующие панели

### 3.1. Иерархия видов

```
Workspace::render_banner_and_active_tab                (workspace/view.rs:22606)
└─ Flex::row  main_content
   ├─ левые панели из header_toolbar config (ToolsPanel = LeftPanelView, TabsPanel)   (:22644-22652)
   ├─ Shrinkable(terminal_content)  ← баннер + ChildView(active_tab.pane_group)     (:22613-22627, :22654-22661)
   ├─ правые панели (CodeReview = RightPanelView)                                   (:22664-22690)
   └─ (вертикальные вкладки: панели рисуются в render_panels, :23433-23497)
PaneGroup::render                                      (pane_group/mod.rs:8345)
└─ Stack
   └─ Flex::column
      ├─ баннер неподдерживаемого шелла
      └─ Shrinkable(1., дерево панелей | развёрнутая панель)   (:8360-8374)   ← ДОК ВСТАВЛЯЕТСЯ ЗДЕСЬ
```

**Рекомендация: док рендерит `PaneGroup`.** Fork-only файл `app/src/pane_group/warpium_bottom_dock.rs` (`impl PaneGroup { fn render_warpium_bottom_dock(&self, app) -> Option<Box<dyn Element>> }`), а в `render` добавляется одна строка после `:8374`. По ширине это совпадает с JetBrains: нижний tool window занимает пространство между левой и правой боковыми панелями, а не всё окно. Почему не workspace-уровень, как `LeftPanelView`:

- док **владеет** панелями вкладки, и их `ChildView` (`PaneId::render`, `pane_group/pane/mod.rs:472`) естественно рендерит та же `PaneGroup`;
- per-tab состояние не нужно синхронизировать при переключении вкладок, как сейчас делают `toggle_left_panel` и `ResizableData` (`workspace/view.rs:9752-9806`);
- события resize-перетаскивания уже приходят в `PaneGroup` (`PaneGroupAction::ResizeMove`, `mod.rs:8364-8371`), но для дока лучше готовый `Resizable` (ниже).

### 3.2. Как сделаны боковые панели (что переиспользовать)

- **Состояние на вкладке, вид на workspace.** `PaneGroup::{left_panel_open, right_panel_open, is_right_panel_maximized}` (`mod.rs:929-934`); `LeftPanelView` / `RightPanelView` — по одному на окно (`workspace/view.rs:1243-1245`, создание `:3195-3215`).
- **Ширина.** `ResizableData` с `ModalType::{LeftPanelWidth, RightPanelWidth}` (`app/src/terminal/resizable_data.rs:16-36`) на окно; панель рендерится через `Resizable` с `DragBarSide` (`workspace/view/right_panel.rs:12-15`, `:425-467`, `:1887`). У `DragBarSide` уже есть `Top` (`crates/warpui_core/src/elements/gui/resizable.rs:182-188`), это ровно ручка нижней панели.
- **Сохранение.** `TabSnapshot { left_panel: Option<LeftPanelSnapshot>, right_panel: Option<RightPanelSnapshot>, … }` (`app/src/app_state.rs:74-89`, `:338-349`). Снапшоты собирают `compute_left_panel_snapshot` и `compute_right_panel_snapshot` (`workspace/view.rs:12164-12204`). В SQLite они лежат JSON-строками в таблице `panels(tab_id, left_panel, right_panel)` (`app/src/persistence/sqlite.rs:1084-1099`, `crates/persistence/src/schema.rs:320-324`), при загрузке читаются в `sqlite.rs:2583-2613`.
- **Разделители между панелями.** `add_panel_with_separator` / `render_panel_separator` (`workspace/view.rs:22644-22690`). Для дока нужен горизонтальный аналог в том же стиле.

### 3.3. Что требуют UI-гайдлайны (`gui-ui-guidelines`)

Сейчас в каталоге одно правило: **переиспользовать темы кнопок**. Для дока это значит:

- кнопки заголовка дока («свернуть», «развернуть», «закрыть», «+»), «Move to panel» в меню и кнопки чипов используют существующие `ActionButtonTheme` из `app/src/view_components/action_button.rs` (`NakedTheme` для иконок в заголовке, `SecondaryTheme` для текстовых);
- никаких `FooDockTheme`, обёрток над `PrimaryTheme` с одной изменённой функцией, `ColorU::new(...)` и `should_opt_out_of_contrast_adjustment`;
- цвета статуса чипов берутся из `appearance.theme()` (accent, ansi green/red, `font_color(bg)`);
- значок со статусом не изобретать: есть `render_icon_with_status` / `IconWithStatusVariant` (`app/src/ui_components/icon_with_status.rs:125-189`), его уже используют вертикальные вкладки (`workspace/view/vertical_tabs.rs:276-286`);
- если какая-то тема покажется «неправильной» для дока, сначала спросить владельца, а не править общую тему.

---

## 4. Вопрос 3: флаги, персистентность, клавиши, палитра, тулбар, чипы

### 4.1. Флаг

В форке два слоя гейтинга:

- **Runtime-настройки `WarpiumSettings` + канал OSS.** Так сделаны `warpium.project_toolbar`, `warpium.android` и другие (`app/src/warpium/sync_settings.rs:84-112`, `is_enabled` в `app/src/warpium/android.rs:185-189`, `is_warpium_channel` в `app/src/warpium/mod.rs:127-131`).
- **`FeatureFlag` из upstream** (skill `add-feature-flag`, `AGENTS.md` «Feature Flags»).

Рекомендация: оба слоя, по очереди.

1. **На время разработки — `FeatureFlag::WarpiumBottomPanel`.** Шаги: `crates/warp_core/src/features.rs` (вариант enum), `app/Cargo.toml` `[features] warpium_bottom_panel = []` (не в `default`), `#[cfg(feature = "warpium_bottom_panel")] WarpiumBottomPanel` в `app/src/lib.rs`, затем добавить в `DOGFOOD_FLAGS`. В коде только `FeatureFlag::WarpiumBottomPanel.is_enabled()`, без `#[cfg]`. Это нужно потому, что PR 1–3 правят upstream-пути (`close_pane`, снапшот, рендер), и выключенный флаг должен гарантировать старое поведение байт в байт.
2. **Перед раскаткой — настройка `warpium.bottom_panel`** (default on, Settings → Features → Warpium, «Enable/Disable bottom panel» в палитре, keymap-флаг `WarpiumBottomPanel`) по образцу `project_toolbar`. Флаг включается в `default` и затем удаляется скиллами `promote-feature` / `remove-feature-flag`.

Все `EditableBinding` дока получают `.with_enabled(|| FeatureFlag::WarpiumBottomPanel.is_enabled())`, как требует скилл.

### 4.2. Модель и персистентность

```rust
// app/src/pane_group/warpium_bottom_dock.rs (fork-only)
pub(crate) struct BottomDock {
    entries: Vec<DockEntry>,   // порядок подвкладок
    active: Option<PaneId>,
    collapsed: bool,           // свёрнут в полосу чипов
    height: ResizableStateHandle,
}
pub(crate) struct DockEntry {
    pane_id: PaneId,
    origin: DockOrigin,        // Manual | ToolbarButton { id } | Logcat | AndroidRun
    seen_output_generation: u64,
    title_override: Option<String>,
}
```

Чистая логика (порядок, выбор активной после закрытия, повторное использование подвкладки кнопки тулбара, сериализация снапшота) выносится в protoc-free крейт `crates/warpium_bottom_panel`, как это сделано в `warpium_project_toolbar` и `warpium_project_root`. Тогда её можно тестировать `cargo test -p warpium_bottom_panel` в контейнере, где нет `protoc`.

**Снапшот.** Новое поле `TabSnapshot::warpium_bottom_panel: Option<BottomPanelSnapshot>`:

```json
{ "height": 260, "collapsed": false, "active": 1,
  "entries": [
    { "kind": "terminal", "uuid": "…hex…", "cwd": "/repo", "title": "gradle", "origin": {"toolbar": "assemble"} },
    { "kind": "logcat" } ] }
```

- **Хранение.** JSON в новой колонке `panels.warpium_bottom_panel TEXT` (миграция `crates/persistence/migrations/2026-11-…_add_warpium_bottom_panel_to_panels`). Таблица `panels` уже хранит JSON на вкладку, так что правка одна, по образцу `left_panel/right_panel`. Условие записи строки `panels` (`sqlite.rs:1084`) расширяется: `|| tab.warpium_bottom_panel.is_some()`.
- **Почему не в дерево.** `snapshot_for_node` пропускает все скрытые панели (`mod.rs:2147-2154`). Запись лишних корней в `pane_nodes` сломала бы `read_node`, на это предупреждает комментарий в `app_state.rs:165-173`.
- **Восстановление терминала.** Тем же `restore_pane_leaf` (`mod.rs:1610-1660`) из синтезированного `LeafContents::Terminal(TerminalPaneSnapshot { uuid, cwd, … })`. Блоки найдутся по `PaneUuid` в `block_lists` (`app_state.rs:31`), затем `attach_*_off_tree` и запись в док. Процессы, как и везде в Warp, не воскрешаются: восстанавливаются история блоков и cwd. Для `origin = toolbar` подвкладка может предложить «Run again» (без автозапуска: команды команды требуют trust).
- **Logcat.** `kind: "logcat"` пересоздаётся как новая `LogcatPane` (стрим и так начинается пустым, `app_state.rs:159-160`).
- **Зависимость от PR #25.** PR #25 меняет `TabSnapshot` (`pinned_explorer_root` → `project_root: ProjectRoot`) и добавляет миграцию `2026-11-16-000000_add_project_extra_roots_to_tabs`. PR 3 этой фичи нужно делать после мержа #25: дата миграции позже, правки в `TabSnapshot`/`sqlite.rs` поверх. Корень проекта для дока (cwd для `target = "panel"`, «новый терминал в корне») брать из `PaneGroup::project_root()` (PR #25), а не из `pinned_explorer_root`.

### 4.3. Клавиши и палитра

| Действие | Binding name | Mac по умолчанию | Контекст |
|---|---|---|---|
| Показать/скрыть нижнюю панель | `warpium:toggle_bottom_panel` | `cmd-j` (свободна в `app/src`, `crates`; как «Toggle Panel» в VS Code) | `Workspace` |
| Переместить панель в нижнюю панель | `pane_group:move_pane_to_bottom_panel` | без клавиши | `PaneGroup` & не док & `visible_pane_count > 1` или есть замена |
| Вернуть в раскладку | `pane_group:restore_pane_from_bottom_panel` | без клавиши | фокус в доке |
| Следующая / предыдущая подвкладка | `warpium:bottom_panel_next_tab` / `_prev_tab` | `cmd-alt-]` / `cmd-alt-[` (проверить коллизии в PR 2) | фокус в доке |
| Развернуть док на вкладку | `warpium:maximize_bottom_panel` | `cmd-shift-j` | фокус в доке |
| Новый терминал в доке | `warpium:new_terminal_in_bottom_panel` | без клавиши | `Workspace` |

Палитра: «Toggle bottom panel», «Move pane to bottom panel», «Restore pane to layout», «New terminal in bottom panel», «Enable/Disable bottom panel». Регистрация — fork-only `init_bindings` по образцу `app/src/warpium/project_root.rs` из PR #25 (`EditableBinding::new(...).with_group(...).with_context_predicate(...)`).

Пункты меню:

- **«Move to bottom panel»** в контекстном меню терминала, рядом с «Close pane» (`app/src/terminal/view.rs:18348-18370`). Это правка upstream-файла в одну строку `items.extend(warpium::bottom_panel_menu_items(...))`.
- **«Move to layout», «Move to tab…», «Close»** в меню подвкладки дока.

### 4.4. Тулбар: `target = "panel"`

- **Крейт `warpium_project_toolbar`.**
  - `CommandTarget::Panel` с парсингом `"panel"`, сообщение об ошибке со списком целей, `as_str` (`crates/warpium_project_toolbar/src/config.rs:93-125`);
  - подпись «bottom panel» в trust-промпте (`trust.rs:258-262`);
  - пример в `TEAM_TEMPLATE` (`template.rs`);
  - выбор в редакторе тулбара (`app/src/warpium/project_toolbar_editor.rs`);
  - тесты в `config_tests.rs`, `trust_tests`, `edit_tests.rs`.
- **Запуск.** Новая ветка в `run_project_toolbar_command_with_prompts` (`app/src/workspace/view/warpium_project_toolbar.rs:398-487`) рядом с `SplitRight | SplitDown` (`:437-446`). Она вызывает `pane_group.add_terminal_pane_to_bottom_panel(cwd, env, origin = ToolbarButton{id}, title)` по образцу `add_terminal_pane_for_toolbar_command` (`app/src/pane_group/warpium_project_toolbar.rs:13-37`), только с `attach_*_off_tree` вместо `add_pane`.
- **Повторный запуск кнопки (JetBrains Run).** Если в доке есть подвкладка с этим `button id`:
  - она свободна (не `is_long_running`, `view.rs:9608-9614`) — команда выполняется в ней, подвкладка активируется;
  - занята — создаётся новая подвкладка «label (2)».

  Опция `panel_reuse = "rerun" | "new"` — позже, отдельным полем, если понадобится. `cwd`/`env`/переменные работают как у `new_tab`: шелл новый, обёртка не нужна.
- **Фокус.** `target = "panel"` раскрывает док (если свёрнут) и выбирает подвкладку, **фокус не забирает**, как Logcat в `open_android_logcat(focus: false)`.
- **WARPIUM.md** («Project toolbar → Config/Running a command») дополняется целью `panel`.

### 4.5. Чипы статуса (свёрнутая полоса)

Свёрнутый док — полоса высотой с заголовок, в ней чип на каждую подвкладку; клик раскрывает док на этой подвкладке. Содержимое чипа:

| Поле | Источник |
|---|---|
| Иконка | тип: терминал / Logcat / Android (`WarpIcon`, через `render_icon_with_status`) |
| Имя | `title_override` (label кнопки тулбара) → заголовок панели (`PaneConfiguration`, тот же, что в шапке; у долгих команд это команда, ср. `vertical_tabs.rs:4275-4291`) → «Terminal» |
| Состояние | `is_long_running()` (`view.rs:9608-9614`) — «running» (accent, анимированная точка); иначе последний завершённый блок: `Block::exit_code()` (`app/src/terminal/model/block.rs:2782`) — ✓ 0 (зелёный) / ✗ N (красный); Logcat — online/offline устройства из `AndroidModel` |
| Непрочитанный вывод | счётчик-бейдж «●» или число новых строк, пока подвкладка не видна |

**Непрочитанный вывод.** Готового источника нет: `has_unread_for_terminal_view` в вертикальных вкладках — это AI-уведомления (`app/src/ai/agent_management/notifications/item.rs:210-214`). Предлагается монотонный счётчик `output_generation: u64` в `TerminalView`, который увеличивается в `handle_terminal_wakeup` (`view.rs:10237`). Это три строки в upstream-файле: поле, инициализация, инкремент. Док сравнивает его с `DockEntry::seen_output_generation` при рендере, а при показе подвкладки запоминает текущее значение. Альтернатива без правки upstream: опрос раз в секунду числа строк активного блока под `model.lock()`. Он хуже: лишний таймер и захват `FairMutex`, который делит с event loop.

Шелл, который вышел (`exit`), панель закрывает (`terminal_pane.rs:979-993`). Поэтому состояния «процесс завершился» у чипа терминала нет: есть «команда завершилась с кодом N» при живом шелле.

---

## 5. Вопрос 4: план по PR, риски, тесты

### 5.1. Фазы

**PR 1. Модель дока и Move/Restore, за `FeatureFlag::WarpiumBottomPanel`.**

- Новый крейт `crates/warpium_bottom_panel` (чистая модель `BottomDock`: порядок, активная, удаление, reuse по origin).
- `app/src/pane_group/warpium_bottom_dock.rs`:
  - `move_pane_to_dock(id)`: `panes.remove`, `handle_pane_count_change`, фокус на соседа через `focus_next_terminal_pane_and_activate_session(id, PaneRemovalReason::Move, …)`;
  - `restore_pane_from_dock(id)`: `panes.split(base, id, Direction::Down)` / `split_root`;
  - `close_docked_pane`.
- Ветки в `close_pane` для панелей из дока; политика «последняя видимая панель» (новый терминал); защита split/resize/навигации; `reveal_and_focus_pane` раскрывает док.
- Минимальный рендер: одна строка в `PaneGroup::render`, док фиксированной высоты, активная подвкладка через `PaneId::render`, простая полоса вкладок.
- Пункт «Move to bottom panel» в меню терминала, «Toggle bottom panel» и `cmd-j`.
- Тесты: unit по модели; `App::test` на `PaneGroup`: move → PTY-модель жива (`terminal_manager` handle тот же, `pane_contents` содержит id), `visible_pane_count` уменьшился; `exit` в доке не закрывает вкладку. Интеграционный тест: `echo` в спрятанной панели продолжает накапливать вывод.

**PR 2. UI дока.**

- `Resizable` с `DragBarSide::Top`; высота на вкладку плюс последняя высота как дефолт, как `LeftPanelWidth`.
- Подвкладки с закрытием и переименованием, свёрнутая полоса чипов (раздел 4.5), счётчик `output_generation`.
- «Развернуть», «Следующая/предыдущая», «Новый терминал в доке».
- Все кнопки через существующие темы (раздел 3.3).
- VM-проверка через warpium-mac: скриншоты свёрнутого и раскрытого дока в светлой и тёмной темах, профиль CPU с 3–5 болтливыми скрытыми процессами.

**PR 3. Персистентность (после мержа PR #25).**

- `TabSnapshot::warpium_bottom_panel`, миграция `panels.warpium_bottom_panel`, запись и чтение в `sqlite.rs`, восстановление через `restore_pane_leaf` вне дерева.
- Тесты: `sqlite_tests.rs` (round-trip, отсутствие колонки или JSON = None, битый JSON не роняет вкладку); интеграционный тест в `session_restoration.rs`: блоки панели из дока восстанавливаются.

**PR 4. `target = "panel"`.** Крейт тулбара, запуск, reuse по `button id`, trust-подпись, редактор, `TEAM_TEMPLATE`, WARPIUM.md. Тесты: `cargo test -p warpium_project_toolbar`, VM-прогон кнопки `./gradlew assemble` с `target = "panel"`.

**PR 5. Logcat и Android Run в доке.**

- `open_android_logcat` (`workspace/view/warpium_android.rs:29-84`) по умолчанию открывает Logcat в доке активной вкладки вместо `Direction::Down` (`:66-74`). Настройка `warpium.logcat_location = "panel" | "split"`.
- Инвариант «одна Logcat на окно» (`LogcatPaneManager`, `:43`) на первом шаге сохраняется: «Open Logcat» в другой вкладке переносит существующую панель в её док через `remove_pane_for_move` → `adopt_into_dock`. Это безопасно: `LogcatPane::detach` снимает регистрацию, `attach` ставит заново (`pane_group/pane/warpium_logcat_pane.rs:54-101`).
- Android Run (`run_android_app`, `:87`) получает опцию запускать `./gradlew :m:installDebug` в доке вместо текущей сессии (подвкладка «Run: app» с reuse).

**PR 6. Перенос между вкладками, drag & drop, раскатка.**

- «Move to tab…» из меню подвкладки; перетаскивание подвкладки на полосу вкладок (тот же `DroppedOnTabBar`, `workspace/view.rs:17172-17215`) и панели из раскладки на полосу дока.
- Настройка `warpium.bottom_panel`, промоушен флага, раздел в WARPIUM.md, короткий абзац в README.

### 5.2. Риски

| Риск | Где | Митигация |
|---|---|---|
| Закрытие последней видимой панели или `exit` в доке закрывает вкладку с живыми процессами | `mod.rs:4871-4881`, `terminal_pane.rs:979-993` | ранние ветки в `close_pane`; unit- и интеграционный тест; существующий `test_tab_closes_when_last_visible_pane_closed` остаётся зелёным при пустом доке |
| Фокус или action-ы по панели вне дерева | `focus_state`, `PaneGroupAction::Add/Resize*` | брать последнюю панель дерева; тесты на split при фокусе в доке |
| Код upstream, который считает `pane_contents == дерево` | обходы `panes_of::<T>()` | аудит в PR 1 (`grep panes_of\|pane_contents.keys`); upstream уже различает их из-за дочерних агентов (`mod.rs:7671-7676`) |
| Конфликты при синке с upstream | `pane_group/mod.rs` (8 тыс. строк), `terminal/view.rs`, `sqlite.rs` | правки upstream-файлов — однострочные вызовы в fork-only модули; список в WARPIUM.md «After merging upstream Warp» |
| Сохранение: потерянные блоки панелей из дока | `sqlite.rs:915-935` переписывает таблицы панелей целиком | проверить в PR 3, как чистятся блоки без `pane_leaves`; блоки панелей из дока должны выжить (round-trip тест) |
| Производительность многих болтливых скрытых процессов | раздел 2.5 | замер в VM; при необходимости не делать `notify` для невидимой подвкладки |
| Пересечение с PR #25 (схема `TabSnapshot`, `pinned_explorer_root` → `project_root`) | `app_state.rs`, `sqlite.rs`, миграции | PR 3 и 4 строго после мержа #25; PR 1–2 этих файлов не трогают |
| Logcat на окно против дока на вкладку | `LogcatPaneManager` | перенос существующей панели в док активной вкладки; позже Logcat на вкладку, отдельным решением |

### 5.3. Тестовая стратегия

- **Unit (`rust-unit-tests`).** Вся логика дока в `crates/warpium_bottom_panel` (`*_tests.rs` рядом), без приложения:
  - активная подвкладка после закрытия (первая, последняя, единственная);
  - reuse подвкладки кнопки (свободна → та же, занята → новая с суффиксом);
  - round-trip сериализации снапшота, старый или пустой JSON;
  - `seen_output_generation` и бейдж.

  В `app` — `App::test` с `initialize_app_for_terminal_view`: move/restore сохраняют `TerminalManager` (тот же `EntityId`), `visible_pane_ids` без панели из дока, `close_pane` панели из дока при одной видимой панели не шлёт `Event::Exited`. Проверять состояние, а не последовательность вызовов. `FeatureFlag::WarpiumBottomPanel.override_enabled(true)` для включения. Плюс `cargo test -p warpium_project_toolbar` на `target = "panel"`.
- **Интеграция (`gui-integration-test`).** Новый модуль `crates/integration/src/test/warpium_bottom_panel.rs`, регистрация в `src/test.rs`, `bin/integration.rs` и `tests/integration/ui_tests.rs`. Сценарии, по одному на тест:
  1. split → long-running `for i in $(seq 1 50); do echo $i; sleep 0.1; done` → Move to panel → assert «вывод продолжает расти» (последняя строка блока достигает 50) и «в раскладке одна видимая панель»;
  2. `exit` в панели из дока не закрывает вкладку;
  3. закрытие последней видимой панели при непустом доке оставляет вкладку и открывает новый терминал;
  4. (PR 3) восстановление: блоки панели из дока на месте после рестарта, по образцу `session_restoration.rs`.

  Assertions именованные, без sleep, через polling.
- **VM (warpium-mac MCP).** `repo_sync(branch)` → `build()` (или CI-артефакт через `install_artifact`, учитывая диск) → `app_launch` → `warpctrl`/`press_keys` (`cmd-j`, меню) → `screenshot` в свёрнутом и раскрытом состоянии, светлая и тёмная темы → `app_logs` (нет warning `reveal_and_focus_pane … off-tree`, нет `report_error!`) → `app_quit`. Отдельный прогон: кнопка тулбара `target = "panel"` с `./gradlew` на Android-проекте и Logcat в доке. Во время исследования сервер `warpium-mac` был недоступен (ECONNREFUSED), поэтому VM-проверки в этот документ не вошли.

---

## 6. Открытые вопросы владельцу

1. Политика при закрытии последней видимой панели, когда док не пуст: новый терминал в корне проекта (рекомендуется) или поднять активную подвкладку дока в раскладку?
2. Восстановление «Move to layout»: рядом с текущей панелью (как JetBrains) или запоминать прежнее место (`restore_hint`)?
3. Logcat: оставить одну на окно и переносить её в док активной вкладки (PR 5) или сразу сделать Logcat на вкладку?
4. `cmd-j` как клавиша по умолчанию устраивает? (JetBrains-раскладка: `alt-f12` для терминала, `cmd-4` для Run.)
5. Нужен ли сплит внутри дока (две подвкладки рядом), или достаточно одной видимой подвкладки?
