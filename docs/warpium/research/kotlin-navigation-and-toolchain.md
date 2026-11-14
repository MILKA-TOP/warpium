# Kotlin-навигация, тулбар проекта и тулчейн Gradle/JDK/SDK в Warpium

Исследование от 2026-11-14. Ветка `master` (база `abb3fe33e`). Продуктовый код не писался.

---

## 1. TL;DR

1. **Главная рекомендация по Kotlin — гибрид, и начинать нужно с Rust-индекса, а не с LSP.** Официальный JetBrains `kotlin-lsp` даёт точную семантику, но остаётся в Alpha. Он занимает 2–5 ГБ RAM, требует полного Gradle-импорта до ответа на `initialize` (2–5 минут на среднем Android-проекте), весит около 400 МБ и **пока не поддерживает KMP**, а ваши основные проекты как раз KMP. Rust-индекс на tree-sitter строится за секунды и отвечает за миллисекунды. Через него проходит 80% ежедневной навигации: перейти к классу, символу или файлу, go-to-definition по импортам, приблизительные usages.
2. **Замеры здесь же, в контейнере, на `ast-index` (тот же класс решения).** На `thunderbird-android` (3 833 файла, 160 модулей, 49 тыс. символов, 182 тыс. ссылок) полный индекс строится за 5,1 с. Поиск класса занимает 3,5 мс, usages 0,65 мс, инкрементальное обновление одного файла 136 мс, база 46 МБ. Для сравнения: Gradle sync проекта на 300 модулей в IDE занимает 47–84 с, и это ещё до индексации.
3. **Половина инфраструктуры в Warpium уже есть:**
   - грамматики Kotlin и Java слинкованы через `arborium`;
   - есть пул rayon и паттерн параллельного парсинга (`crates/ai/src/index/file_outline/native.rs`);
   - есть watcher с gitignore и force-include (`repo_metadata::DirectoryWatcher`);
   - есть FS-кэш дерева файлов, палитра с data sources и UI find-references (`app/src/code/find_references_view.rs`).

   Нужен новый крейт `crates/code_index` плюс палитра «Go to Class/Symbol».
4. **План.**
   - Фаза 1: индекс деклараций и палитра Go to Class/Symbol.
   - Фаза 2: index-based go-to-definition и приблизительный Find Usages, граф Gradle-модулей, generated-код, R-ресурсы, `expect`/`actual`.
   - Фаза 3: `kotlin-lsp` как ленивый, тёплый, опциональный sidecar для diagnostics, completion, rename и точных references.
   - Фаза 4: навигация по библиотекам и SDK, модель проекта из Gradle, опционально jdtls.
5. **Тулбар проекта.** Конфиг лежит в коммитимом `.warpium/toolbar.toml`, личные оверрайды в `~/.warp-oss/projects/<id>/toolbar.toml`. Кнопка — это `run`, `workflow`, `tab_config`, `launch_config`, `url` или `action` из каталога warpctrl. Обязательно trust-подтверждение для команд из репозитория.
6. **Тулчейн.** Позиционирование — «terminal-first IDE, которая **читает** конфиг проекта и Android Studio, а не владеет им». Сейчас нужна только read-only панель «Toolchain»: какой JDK и SDK используется и откуда это взялось. Плюс один per-user override, который применяется к env терминала, LSP и run-конфигам. Главная практическая выгода: `./gradlew` в терминале Warpium получает тот же JDK, что и Gradle в Android Studio, и использует **тот же daemon** вместо второго на 2+ ГБ. Не нужны редакторы signing, SDK Manager, AVD Manager и редактор `gradle.properties`.

---

## 2. Тема 1: Kotlin и навигация «как в проиндексированном проекте»

### 2.1. Как индексирует IntelliJ / Android Studio и что реально сделать быстрее

Конвейер в IntelliJ состоит из двух этапов:

- **Gradle sync.** Tooling API и кастомные модели AGP строят модель проекта: модули, source sets, зависимости, generated-директории.
- **Индексация.** File-based indexes, IdIndex (слово → файлы, префильтр для Find Usages) и **stub index**. Stub — это сериализованное «лёгкое» дерево деклараций файла, по которому работают Go to Class/Symbol без полного парсинга. Точный resolve и Find Usages работают поверх PSI: IdIndex отбирает кандидатов, потом идёт resolve каждого вхождения.

Медленно здесь в первую очередь sync. По данным Gradle, медиана IDE sync для проекта на 300 подпроектов составляет 84 с, а с Isolated Projects 47 с. У Square на проектах с 3 000+ модулями sync сократился с 9 до 3,5 минуты ([Gradle blog](https://blog.gradle.org/introducing-isolated-projects), [Square](https://developer.squareup.com/blog/keeping-ide-sync-times-at-bay-a-historical-perspective/)). Shared indexes экономят до 75% времени индексации Java-проекта ([JetBrains](https://jetbrains.com/help/idea/shared-indexes.html)).

**Чего реально достичь.** Точный type resolution быстрее IntelliJ мы не сделаем, и не нужно. Зато можно отвязать навигацию от sync. Stub index и IdIndex воспроизводятся на tree-sitter без Gradle и без JVM за секунды. Точная семантика остаётся за тёплым LSP, который грузится в фоне и переживает рестарты.

### 2.2. Варианты

**JetBrains `Kotlin/kotlin-lsp`** ([repo](https://github.com/Kotlin/kotlin-lsp), [docs](https://kotlinlang.org/docs/kotlin-lsp.html), [releases](https://github.com/Kotlin/kotlin-lsp/releases)):

- **Статус и движок.** Alpha. Построен на IntelliJ, Kotlin-плагине, AIR и Fleet.
- **Версии.** Активные релизы: v262 → v263 → v271.2085.0 (12 ноября).
- **Импорт проектов.** Gradle и Maven. Android через AGP добавлен в v262.4739 как *experimental*. **KMP не поддерживается** («coming in future releases»).
- **Дистрибутив.** Zip с `bin/intellij-server` около 400 МБ (было 600), есть Homebrew и расширение VS Code.
- **JVM.** Для запуска нужен JDK 25. JRE свой: при отсутствии `JAVA_HOME` Gradle-импорт падает обратно на него, а с v271 импорт уважает `JAVA_HOME`.
- **Индекс и кэш.** Индекс хранится в RocksDB. Модель workspace с v262.7569 сохраняется между рестартами, что ускоряет холодный старт.
- **Лицензия.** Apache-2.0, но сервер «partially closed-source». Репозиторий — read-only зеркало.
- **Производительность** (официальных бенчмарков нет):
  - RSS 2,6 ГБ плюс второй процесс 1,8 ГБ на маленьком репо; 1,7 + 1,3 ГБ на 7-модульном. До примерно 2 минут после старта ответы неверные. `workspaceSymbol` ненадёжен, а definition, references и hover «сильные», в том числе кросс-языково ([mvysny, сентябрь 2026](https://mvysny.github.io/java-kotlin-lsp-status/)).
  - На Android-проекте из 760 файлов и 50+ зависимостей сервер не отвечает на `initialize`, пока не закончится Gradle sync, а это 2–5 минут ([opencode #28289](https://github.com/anomalyco/opencode/issues/28289)).
  - Кэш вырастал до 150 ГБ на проекте с 300 тыс. строк Kotlin. Исправлено ([kotlin-lsp #213](https://github.com/Kotlin/kotlin-lsp/issues/213)).

**fwcd/kotlin-language-server** ([repo](https://github.com/fwcd/kotlin-language-server)):

- В README проект **объявлен deprecated** в пользу официального сервера, автор Kotlin больше не использует.
- Известные проблемы: рост памяти с 1 до 4–5 ГБ, медленная инициализация из-за индексации всех Gradle-зависимостей, на Android-проектах больше 3 минут загрузки ([#376](https://github.com/fwcd/kotlin-language-server/issues/376), [#510](https://github.com/fwcd/kotlin-language-server/issues/510)).
- Инвестировать в него не стоит.

**Rust-native tree-sitter индекс** (по образцу `ast-index`, [repo](https://github.com/defendend/Claude-ast-index-search)):

- `ast-index` — Rust, tree-sitter, SQLite с FTS и bm25. Хранит символы, ссылки, импорты, модули, наследование, граф Gradle-модулей, XML и ресурсы. Ссылки не резолвятся по типам: каждой паре присваивается уровень уверенности (local, import, unique, ambiguous).
- Авторские цифры на Android-проекте примерно из 29 тыс. файлов: `class` 1 мс, `usages` 8 мс, `search` 11 мс; rebuild 40 тыс. файлов за 5,5 с; update 20 файлов за 0,4 с.
- Есть также **kmp-lsp** ([repo](https://github.com/Hessesian/kmp-lsp)) — LSP-сервер на Rust и tree-sitter для Kotlin, Java и Swift, без JVM, заявлено меньше 200 МБ. Полезен как референс эвристик resolve. MIT, около 200 звёзд.

**Собственные замеры** в этом контейнере (Linux, 12 vCPU, `ast-index` 3.29.0):

| Корпус | Файлы / модули | Символы / ссылки | Rebuild | БД | `class` | `usages` | `search` | update 1 файла |
|---|---|---|---|---|---|---|---|---|
| `android/nowinandroid` | 317 / 36 | — | 0,28 с | 2 МБ | 1,8 мс | 0,3 мс | 7,5 мс | 18 мс (no-op) |
| `thunderbird/thunderbird-android` | 3 833 / 160 | 48 942 / 182 008 | 5,1 с* | 46 МБ | 3,5 мс | 0,65 мс | 19 мс | 136 мс |

\* Включая 71 тыс. ресурсов, XML-usages и транзитивные зависимости модулей. Чистый проход по Kotlin/Java будет заметно быстрее.

### 2.3. Сравнение

| | Скорость: старт / первый ответ | Латентность навигации | Память | Точность | Зрелость | Android / Gradle / KMP | Лицензия | Усилия интеграции в Warpium |
|---|---|---|---|---|---|---|---|---|
| **JetBrains kotlin-lsp** | Холодный старт — минуты (ждёт Gradle sync); тёплый лучше (модель персистится) | Definition и references хорошие после прогрева; workspaceSymbol слабый | 2–5 ГБ, диск до ГБ | Высокая (компилятор Kotlin) | Alpha, частые релизы | Gradle да; AGP experimental; **KMP нет** | Apache-2.0, частично закрытый | Средние: новый `LSPServerType`, загрузка zip, env и JDK, долгий `initialize`, `workspace/configuration` |
| **fwcd/kotlin-language-server** | Минуты на Android | Нестабильно | 1–5 ГБ | Средняя | **Deprecated** | Слабо | MIT | Средние; не рекомендуется |
| **Rust tree-sitter индекс** | Секунды холодно, ≤0,3 с тёпло | 1–20 мс | 50–200 МБ | Имена и импорты без типов: классы почти точно, члены неоднозначно | Своя разработка; референсы ast-index и kmp-lsp | Kotlin, Java, KMP source sets, Gradle-модули из скриптов без Gradle | Своя (AGPL/MIT форка) | Средние: новый крейт; грамматики, пул, watcher и UI уже есть |
| **Гибрид (рекомендуется)** | Индекс мгновенно, LSP догоняет в фоне | Индекс сразу, LSP уточняет | 100 МБ + LSP по требованию | Лучшее из обоих | — | Всё, KMP — за счёт индекса | — | Индекс (фазы 1–2) + LSP (фаза 3) |

### 2.4. Особенности Android-проектов

- **Java + Kotlin.** Индекс парсит обе грамматики. Сейчас у Java есть `identifiers.scm`, у Kotlin его **нет**, поэтому outline для AI на Kotlin сейчас пустой. Это побочный быстрый выигрыш. `kotlin-lsp` резолвит Kotlin↔Java, но Java-сервером не является. Для проектов с тяжёлой Java, вроде Signal, позже можно подключить jdtls.
- **Generated-код.** Директории `build/generated/{ksp,source/buildConfig,data_binding_base_class_source_out,...}` игнорируются через gitignore. Индексировать их нужно выборочно через `DirectoryWatcher::register_force_included_paths`: механизм уже есть, используется в `app/src/lib.rs:2059`. Индекс работает только если сборка уже была.
- **`R`-класс.** Его лучше не индексировать, а синтезировать: `R.string.foo` разрешается сразу в `res/values/strings.xml` (так делает ast-index). Для пользователя это лучше, чем переход в сгенерированный `R.java`.
- **Compose.** Ничего особенного: `@Composable fun` — обычные декларации. `@Preview` удобно помечать в индексе на будущее (roadmap п. 8).
- **Мультимодульность (50–300 модулей).** Граф модулей строится парсингом `settings.gradle(.kts)` (`include`) и `build.gradle(.kts)` (`project(":x")`, `projects.core.data`) тем же tree-sitter Kotlin. Gradle для этого не нужен. Граф используется для ранжирования и фильтрации: кандидат из модуля, недостижимого по зависимостям, получает понижение.
- **KMP.** Source sets `commonMain`, `androidMain`, `iosMain` определяются по путям. Пары `expect`/`actual` связываются по FQN. Этого не умеет ни один существующий Kotlin LSP.

### 2.5. Как это ложится на код Warpium

**LSP (`crates/lsp`).**

- Серверы — это enum `LSPServerType` (`supported_servers.rs`) плюс trait `LanguageServerCandidate` (`language_server_candidate.rs`) с методами detect, install и fetch metadata. Enum персистится в sqlite, поэтому вариант `KotlinLsp` добавляется только в конец.
- Языки задаёт `LanguageId::from_path` (`config.rs`). Сейчас там нет Kotlin и Java, и маршрутизация `server_for_path` исходит из 1:1 язык→сервер.
- Запуск идёт через `persisted_workspace.rs` (`should_suggest_for_repo` → `LspServerConfig::new` → `LspManagerModel::register/start_all`), один набор серверов на workspace root.
- `LspServerConfig` сейчас передаёт только `PATH`. Для Kotlin понадобятся:
  - произвольный env (`JAVA_HOME`);
  - `initializationOptions` и ответ на `workspace/configuration` (`intellij.projects`, `intellij.buildTool`);
  - терпимость к долгому `initialize` с показом прогресса через уже существующий `BackgroundTaskInfo`.
- Изменения файлов уходят серверу через `server_repo_watcher.rs` (`RepositorySubscriber` → `did_change_watched_files`). Это переиспользуется.
- Реализованы запросы `definition`, `references`, `hover`, `format` (`service.rs`). `workspace/symbol` и `documentSymbol` отсутствуют, но в гибриде они и не нужны: их обслуживает индекс.

**Editor и UI.**

- `app/src/code/local_code_editor.rs`: `call_goto_definition`, `fetch_find_references_and_show`, привязки vim `gd`/`gr` и hover-definition. Find-references показывается карточкой `FindReferencesView`.
- Точка интеграции: провайдер навигации с fallback. Сначала запрос к индексу, ответ меньше чем за 20 мс. Если LSP готов, параллельно идёт запрос к нему, и при расхождении результат заменяется с пометкой «уточнено».
- Палитра (`app/src/search/command_palette/*`, `warp_search_core` с data sources, mixer и fuzzy) уже ищет файлы через `FileSearchModel` с кэшем плоского списка репо, который инвалидируется по `FileTreeUpdated`. Новый data source «Symbols» встаёт туда же.

**Индексация и потоки.**

- `repo_metadata` даёт дерево файлов, gitignore-кэш и `DirectoryWatcher` (notify-debouncer-full, на macOS FSEvents) с подпиской `RepositoryUpdate` (added, modified, deleted).
- `crates/ai/src/index/file_outline/native.rs` уже делает почти то, что нужно: параллельный tree-sitter парсинг на выделенном rayon-пуле (`THREADPOOL`) с запретом работы на main thread, `languages::language_by_filename` и `symbols_query`.
- Ripgrep (`warp_ripgrep`) остаётся для полнотекстового поиска и как fallback для usages.

### 2.6. Рекомендуемая архитектура

```
crates/code_index (новый, без зависимостей от app)
 ├─ scanner: обход через repo_metadata (gitignore + force-include generated)
 ├─ extractor: tree-sitter (arborium kotlin/java) + собственные tags-запросы
 │     defs: class/interface/object/enum/typealias/fun/val/var/constructor
 │     header: package, imports (FQN, alias, star), annotations (@Composable/@Preview)
 │     refs: type refs, call names, navigation_suffix → имя (без resolve)
 ├─ gradle_graph: settings.gradle(.kts) + build.gradle(.kts) → модули и рёбра
 ├─ resources: res/values/*.xml, layout/*.xml → R.*-цели
 ├─ store: in-memory (интернированные строки, FST/trigram для fuzzy)
 │         + снапшот на диске ~/.warp-oss/index/<repo-hash>/ (rkyv/bincode),
 │           ключ — (path, mtime, size); ссылки — инвертированный индекс имя→позиции
 ├─ resolver (эвристики): тот же файл → явный импорт FQN → тот же пакет
 │         → star-импорты → дефолтные импорты Kotlin → достижимость модуля
 └─ API: find_symbols(query, kind) / definition_candidates(file, pos) / usages(name, fqn?)
app: data source «Symbols» в палитре; NavigationProvider = Index ⊕ LSP
lsp: LSPServerType::KotlinLsp (ленивый, опциональный, idle-shutdown)
```

**Почему не SQLite, как у ast-index?** Для UI важна латентность одного кадра. Fuzzy по 100–300 тыс. символов в памяти — это единицы миллисекунд и ноль I/O. Снапшот на диске нужен только для тёплого старта. Если нужна выдача для агентов, тот же API можно отдать через MCP: Warpium уже хостит MCP, и вы используете ast-index именно для агентов.

**Модель проекта из Gradle** (init script или Tooling API) **для навигации не нужна.** Она нужна в фазе 4: classpath для библиотек и run-конфиги. `kotlin-lsp` делает свой импорт через Tooling API 9.8 и сам кэширует модель, так что дублировать не нужно. Ускорить его старт можно двумя способами:

- запускать заранее, в фоне, при открытии Kotlin-проекта, если пользователь включил режим «Auto»;
- не убивать при закрытии вкладок, а гасить только по idle-таймауту, например 30 минут.

### 2.7. Поэтапный план и целевые показатели

| Фаза | Объём | Цели (MacBook M-серии, корпус Thunderbird и DDG) |
|---|---|---|
| **0. Бенчмарк** (1–2 дня) | Корпуса с фиксированными SHA; бинарь `crates/code_index/examples/bench.rs` (без сборки app); baseline на ast-index; ручной замер kotlin-lsp на Thunderbird через `crates/lsp/examples` (время до первого `definition`, RSS) | — |
| **1. Индекс деклараций + Go to Class/Symbol** (главный выигрыш) | Kotlin `identifiers.scm`/tags; extractor Kotlin+Java; снапшот; инкрементальное обновление из `DirectoryWatcher`; палитра Go to Class (⌘O), Go to Symbol (⌥⌘O); фильтр по типу и модулю | Холодный индекс ≤2 с на 4 тыс. файлов, ≤5 с на 7 тыс.; **тёплый старт ≤300 мс**; ввод в палитре → результат p95 **≤16 мс** на 300 тыс. символов; обновление после сохранения ≤50 мс; RSS индекса ≤150 МБ; main thread не блокируется дольше 2 мс |
| **2. Definition + Usages без LSP** | Resolver по импортам, пакетам и модулям; ⌘-клик/F12/`gd` через индекс; приблизительный Find Usages (имя + импорт FQN, без комментариев и строк) в `FindReferencesView` с бейджем «≈»; граф Gradle-модулей; generated-директории; `R.*` → XML; `expect`/`actual`; переход к Implementations по супертипам | Go-to-definition p95 **≤20 мс**; usages ≤50 мс на 200 тыс. ссылок; точность на классах ≥95% (сверка с kotlin-lsp на выборке из 200 вызовов) |
| **3. kotlin-lsp sidecar** | `LSPServerType::KotlinLsp` (загрузка zip из GitHub Releases, как у rust-analyzer и clangd); режимы Off / On demand / Auto; env `JAVA_HOME` из Toolchain; `workspace/configuration`; прогресс импорта; idle-shutdown; diagnostics, hover, completion, rename; LSP уточняет результаты индекса | UI никогда не ждёт LSP: ответы индекса показываются сразу. Тёплый рестарт сервера до первого точного definition ≤20 с (зависит от сервера, замерить в фазе 0) |
| **4. Библиотеки и проект** | Индекс `-sources.jar` из `~/.gradle/caches` и `android.jar` (лениво); Gradle init script → JSON (source sets, classpath, generated dirs), кэш по хэшу `*.gradle.kts` и `libs.versions.toml`; опционально jdtls для Java-heavy | Переход в класс библиотеки ≤50 мс после первичной индексации jar |

**Корпуса для бенчмарка:**

| Корпус | Kotlin-файлы | Java-файлы | Модули | Назначение |
|---|---|---|---|---|
| `android/nowinandroid` | 310 | — | 37 | Smoke-тест |
| `thunderbird/thunderbird-android` | 3,5 тыс. | 290 | 161 | Основной корпус |
| `duckduckgo/Android` | 6,6 тыс. | — | 205 | Много модулей |
| `signalapp/Signal-Android` | 5,2 тыс. | 1,6 тыс. | 65 | Тяжёлый смешанный Java/Kotlin |

Плюс один ваш реальный KMP-проект (`kmp-bspb-demo`).

**Метрики:** wall time холодного и тёплого индекса, RSS, p50/p95 по фиксированному списку из примерно 50 запросов (классы, функции, extension-функции, неоднозначные имена) и время обновления после `touch`/правки.

---

## 3. Тема 2: тулбар с кнопками для каждого проекта

**Где лежит конфиг.**

- **Командный:** `<project>/.warpium/toolbar.toml`, коммитится.
  - Warp уже читает проектные `.warp/workflows/*.yaml`, `.warp/.mcp.json` и `.warp/skills`: константа `WARP_CONFIG_DIR = ".warp"` в `crates/warp_core/src/paths.rs` не зависит от канала.
  - Для фич форка лучше отдельная `.warpium/`, чтобы не конфликтовать с возможным будущим апстримным файлом. Workflows при этом продолжаем брать из `.warp/workflows`.
- **Личный:** `~/.warp-oss/projects/<project-id>/toolbar.toml`.
  - `project-id` — канонический путь, а позже запись в `ProjectManagementModel` (`app/src/projects.rs`, уже персистится).
  - Мёрж по `id` кнопки: добавить, скрыть (`hidden = true`) или переопределить поля.
  - Дополнительно глобальный `~/.warp-oss/toolbar.toml` с кнопками для всех проектов.
- **Hot reload** через тот же `DirectoryWatcher`.

**Как определяется проект** для активной панели (первое совпадение):

1. явный проект из будущей фичи «Projects» (roadmap п. 11);
2. закреплённый корень Project Explorer (PR #8);
3. ближайший предок cwd с `.warpium/`;
4. git-корень (`DetectedRepositories`);
5. иначе показываются только глобальные кнопки.

Тулбар переключается при смене активной вкладки или панели.

**Что умеет кнопка** (переиспользуем существующие сущности):

```toml
[[button]]
id = "assemble"
icon = "hammer"            # встроенная иконка warp_core или путь к svg/png в репо
tooltip = "assembleDebug"
run = "./gradlew :app:assembleDebug"
target = "new_tab"         # current | new_tab | split_right | split_down
cwd = "."                  # относительно корня проекта
env = { GRADLE_OPTS = "-Xmx4g" }
confirm = false

[[button]]
id = "logcat"
workflow = "logcat-app"    # .warp/workflows/*.yaml, параметры спрашиваются как обычно

[[button]]
id = "dev-layout"
tab_config = "android-dev" # ~/.warp-oss/tab_configs/*.toml или .warpium/tab_configs/
# launch_config = "Android" — альтернатива для многооконных раскладок

[[button]]
id = "ci"
url = "https://gitlab.example/…/pipelines"

[[button]]
id = "tree"
action = "toggle_project_explorer"   # имя действия из каталога warpctrl (crates/local_control/src/catalog.rs)

# позже: run_config = "app"  (из .idea/runConfigurations / .run/*.run.xml, roadmap п. 8)
```

**Безопасность.** Команды из коммитимого файла — недоверенный код, как `.envrc` или VS Code Workspace Trust. При первом запуске и при каждом изменении хэша файла показывается диалог «Доверять кнопкам проекта X?». Без доверия кнопки видны, но заблокированы.

**UI.** Строка иконок в заголовке окна справа от вкладок или над Project Explorer, плюс overflow-меню. Каждой кнопке можно назначить keybinding (`keybinding = "cmd-shift-r"`). Кнопки также доступны в палитре.

---

## 4. Тема 3: нужен ли Warpium свой конфиг Gradle/JDK/Android SDK

### 4.1. Что где хранится сегодня

| Что | Где | Коммитится | Кто читает |
|---|---|---|---|
| Android SDK, NDK | `local.properties` (`sdk.dir`), env `ANDROID_HOME`/`ANDROID_SDK_ROOT` | нет | AGP |
| JVM для Gradle в Android Studio | `.idea/gradle.xml` → `gradleJvm` (`#GRADLE_LOCAL_JAVA_HOME`, `#JAVA_HOME` или имя JDK); для макроса путь лежит в `.gradle/config.properties` → `java.home` (по умолчанию JBR) ([developer.android.com](https://developer.android.com/build/jdks)) | обычно нет | **Только AS/IDEA** |
| JVM для Gradle из терминала | `JAVA_HOME`, иначе `java` из `PATH`; `org.gradle.java.home` в `gradle.properties` | по-разному | Gradle |
| Критерии JVM демона | `gradle/gradle-daemon-jvm.properties` (Gradle 8.8+, авто-provisioning с 8.13) ([docs](https://docs.gradle.org/current/dsl/org.gradle.buildconfiguration.tasks.UpdateDaemonJvm.html)) | да | Gradle |
| JDK для компиляции | `jvmToolchain(17)` / `java.toolchain` в скриптах; `org.gradle.java.installations.paths` | да | Gradle |
| Аргументы демона и флаги | `gradle.properties` проекта (`org.gradle.jvmargs`, `android.*`, `kotlin.*`); `~/.gradle/gradle.properties` (приоритетнее, туда же кладут секреты) | проектный — да | Gradle |
| Signing | `signingConfigs` в `build.gradle(.kts)`, значения из `~/.gradle/gradle.properties`, env или `keystore.properties` | скрипт — да, секреты — нет | Gradle |
| Run-конфиги | `.idea/runConfigurations/*.xml`, `.run/*.run.xml`, `.idea/workspace.xml` (RunManager) | частично | AS/IDEA |

Ключевой вывод: **почти всё уже описано файлами, которые читает Gradle.** Единственное, что знает только Android Studio, — это «Gradle JDK» из `.idea/gradle.xml` и `.gradle/config.properties`. Отсюда частая проблема: Android Studio запускает Gradle на JBR 21, а терминал на `JAVA_HOME=17`. В итоге получаются два демона по 2+ ГБ и разный кэш конфигурации.

### 4.2. Как это делают лёгкие редакторы

- **VS Code.** Настройки в `settings.json`, без UI-мастеров:
  - Java/Gradle-расширения: `java.jdt.ls.java.home` (JDK для сервера), `java.configuration.runtimes` (список JDK), `java.import.gradle.java.home`;
  - Kotlin-расширение JetBrains уважает `JAVA_HOME` и `intellij.projects[].java-home`.
- **Zed.** `lsp.<server>.binary.env.JAVA_HOME` и `lsp.jdtls.settings.java_home`. Gradle-мост настраивается отдельно ([zed.dev](https://zed.dev/docs/languages/kotlin)).
- **Общий паттерн.** Редакторы читают проект и окружение. Свои настройки — это лишь «каким JDK запускать сервер и импорт», без SDK Manager и signing-мастеров.

### 4.3. Рекомендация

**Позиционирование.** Warpium — terminal-first IDE, которая **читает** существующую конфигурацию (Gradle-файлы, `local.properties`, `.idea/`, env) и **ничего не хранит в проекте сама**, кроме собственных `.warpium/*`. Переопределения бывают только per-user и только когда автодетект ошибся. Дублировать Android Studio (SDK Manager, AVD, Signing wizard, Project Structure) — анти-цель. Для этого есть `sdkmanager`, `avdmanager` и кнопки тулбара.

**Что нужно сейчас** (вместе с фазой 3 и run-конфигами, не раньше):

1. **Read-only панель «Toolchain»** в настройках проекта и в статус-баре. Показывает:
   - JDK для Gradle и его источник: «из `.idea/gradle.xml` → `#GRADLE_LOCAL_JAVA_HOME` → `.gradle/config.properties`», `org.gradle.java.home`, daemon criteria или `JAVA_HOME`;
   - Android SDK (`local.properties`, env или стандартный путь `~/Library/Android/sdk`);
   - версии Gradle wrapper, AGP и Kotlin из `libs.versions.toml`;
   - предупреждение при расхождении «AS использует X, терминал — Y».
2. **Один per-user override** в `~/.warp-oss/projects/<id>/toolchain.toml`: `java_home`, `android_sdk`, `gradle_opts`. Применяется к:
   - env новых терминальных сессий проекта (`JAVA_HOME`, `ANDROID_HOME`);
   - env процесса LSP (kotlin-lsp v271 берёт `JAVA_HOME` для Gradle-импорта; `LspServerConfig` нужно научить передавать env);
   - run-конфигам.

   По умолчанию override = «как в Android Studio». Так терминал разделяет daemon с AS, и в этом главная выгода.
3. **Мягкие фиксы со стандартными записями.** Если нет `local.properties`, предложить создать его с найденным `sdk.dir`. Писать только в стандартные файлы и только по клику.

**Позже или никогда:** UI для `gradle.properties` (открывается как файл в редакторе), управление демонами (`./gradlew --status/--stop` через кнопку), signing (это скрипты и секреты, остаются в Gradle), SDK и AVD Manager.

---

## 5. Открытые вопросы

1. **KMP или Android.** Какие проекты приоритетны для навигации: KMP (`kmp-bspb-demo`, дистрибутивы) или чистый Android (`app-android`)? kotlin-lsp KMP не поддерживает, поэтому для KMP индекс — единственный путь в ближайшие месяцы. Сколько модулей и файлов в самом большом из них?
2. **Ресурсы под kotlin-lsp.** Сколько RAM на вашем Mac? Готовы ли держать 2–5 ГБ под kotlin-lsp и скачивать около 400 МБ? Или по умолчанию режим «On demand»?
3. **Доля Java.** Нужен ли jdtls, или Java в ваших проектах мало?
4. **Имя каталога.** `.warpium/` или переиспользовать `.warp/`? Первое безопаснее при апстрим-синках, второе привычнее команде, которая пользуется Warp.
5. **Хранилище индекса.** Нужен ли индекс агентам (MCP/CLI вместо `ast-index`)? Если да, стоит сразу заложить внешний API, а может быть и SQLite-хранилище, совместимое по смыслу с ast-index.
6. **Тулбар.** Где он должен быть: в заголовке окна или над Project Explorer? Нужен ли «run-widget» с выпадающим списком конфигураций, как в JetBrains, или хватит плоских кнопок?
7. **Корпуса.** Устраивает ли набор Thunderbird + DuckDuckGo + Signal + один ваш проект, или есть внутренний большой проект для замеров (на Mac через warpium-mac MCP)?

---

**Источники:** [kotlin-lsp](https://github.com/Kotlin/kotlin-lsp) · [releases](https://github.com/Kotlin/kotlin-lsp/releases) · [kotlinlang docs](https://kotlinlang.org/docs/kotlin-lsp.html) · [mvysny: состояние Java/Kotlin LSP](https://mvysny.github.io/java-kotlin-lsp-status/) · [opencode #28289](https://github.com/anomalyco/opencode/issues/28289) · [kotlin-lsp #213](https://github.com/Kotlin/kotlin-lsp/issues/213) · [fwcd/kotlin-language-server](https://github.com/fwcd/kotlin-language-server) · [ast-index](https://github.com/defendend/Claude-ast-index-search) · [kmp-lsp](https://github.com/Hessesian/kmp-lsp) · [Gradle Isolated Projects](https://blog.gradle.org/introducing-isolated-projects) · [Square: sync times](https://developer.squareup.com/blog/keeping-ide-sync-times-at-bay-a-historical-perspective/) · [IntelliJ shared indexes](https://jetbrains.com/help/idea/shared-indexes.html) · [Android: Java versions in builds](https://developer.android.com/build/jdks) · [Gradle UpdateDaemonJvm](https://docs.gradle.org/current/dsl/org.gradle.buildconfiguration.tasks.UpdateDaemonJvm.html) · [Zed Kotlin](https://zed.dev/docs/languages/kotlin)
