# Встроенный эмулятор Android и зеркалирование устройств в панели Warpium

Исследование и дизайн от 2026-11-18, пункт 13 роадмапа, фаза B. Ветка `master` (база `d62bf5292`). Продуктовый код не писался, спайков не было. Выводы о Warp сделаны по чтению кода, ссылки даны как `файл:строка` на базе `d62bf5292`. Внешние источники прочитаны 2026-11-18:

- исходники эмулятора: AOSP `platform/external/qemu`, ветка `emu-master-dev`;
- Android Studio: `platform/tools/adt/idea`, ветка `mirror-goog-studio-main` (`EmulatorCommandBuilder.kt` взят из зеркала `JetBrains/android`);
- scrcpy: `master` и релиз v5.0.1 от 2026-10-08;
- документация developer.android.com.

Ранняя заметка [android-emulator-embedding.md](android-emulator-embedding.md) этим документом заменяется. Три её утверждения неверны, исправления даны ниже, в разделах 2.2 и 2.3:

- «Android Studio получает кадры через MMAP»;
- «запускать с `-grpc <port>`»;
- «токен лежит в `~/.emulator_console_auth_token`».

---

## 1. TL;DR

1. **gRPC у эмулятора включён всегда, отдельный флаг не нужен.** В `qemu-setup.cpp:624-632` сервер поднимается при любом запуске Android-образа, если не задан `-ports` без `-grpc`. Без `-grpc` эмулятор сам выбирает порт из диапазона `serial+3000…+1000` (8554 для `emulator-5554`), слушает только loopback и включает **и** статический токен, **и** JWT (`qemu-setup.cpp:386-396`, `:466-490`). Порт и токен он пишет в файл обнаружения `pid_<pid>.ini` (`:523-524`, `:475`). Отсюда следствие: эмулятор, который фаза A уже запускает в отдельном окне (`crates/warpium_android/src/avd.rs:145-154`, просто `emulator -avd X`), **уже доступен Warpium по gRPC**. Первый полезный шаг фазы B — «показать запущенный эмулятор в панели», и для него не нужно менять запуск.
2. **Как это делает Android Studio:** `-qt-hide-window -grpc-use-token -idle-grpc-timeout 300` (`EmulatorCommandBuilder.kt:64-68`, то же в официальной документации command-line). Флаг `-grpc <port>` Studio **не** передаёт. Это важно для безопасности: с явным `-grpc <port>` эмулятор слушает `[::]`, то есть все интерфейсы, и без `-grpc-use-token` работает без авторизации (`qemu-setup.cpp:392-396`, `GrpcServices.cpp:356-360`). Авторизация — заголовок `authorization: Bearer <grpc.token>` (`GrpcServices.cpp:342-346`, `EmulatorController.kt:1672-1678`). Studio раз в 2 минуты делает keepalive через `getVmState` (`EmulatorController.kt:1520-1545`, `:1870`), чтобы не сработал `-idle-grpc-timeout`. Этот же таймаут даёт полезную страховку: если Warpium упадёт, скрытый эмулятор сам завершится через 5 минут.
3. **Кадры.** `streamScreenshot(ImageFormat)` отдаёт кадр только когда меняется фреймбуфер (`EmulatorService.cpp:766-815`). На статичном экране кадров нет, а значит нет ни CPU, ни трафика. Масштабирует эмулятор сам, до запрошенных `width`/`height` (`emulator_controller.proto:1036-1052`). Studio сейчас берёт **RGB888 по gRPC** (`EmulatorView.kt:817-826`), а не MMAP. MMAP или shm есть (`ImageTransport`, proto `:981-1001`; `EmulatorService.cpp:699-704`, `:1032-1050`), но с оговоркой «can result in tearing». Для нас лучший формат — **RGBA8888**: он побайтно совпадает с `MTLPixelFormat::RGBA8Unorm` текстур Warp (`metal/renderer.rs:469`), так что CPU-конвертации нет.
4. **Ввод.** `streamInputEvent` — один упорядоченный client-streaming вызов для мыши, касаний и колеса, им пользуется Studio (`EmulatorController.kt:591-626`). Кнопки отправляются через `sendKey` с W3C-именами `GoBack`/`GoHome`/`AppSwitch`/`Power`/`AudioVolumeUp` (proto `:820-835`, Studio-экшены `EmulatorPushButtonAction("GoBack")`). Поворот — `setPhysicalModel(ROTATION, [0, 0, угол])`, как в `EmulatorRotateAction.kt`. Снимок экрана — `getScreenshot(PNG)`. Звук эмулятор выводит сам, `streamAudio` в MVP не нужен.
5. **Рендеринг в warpui.** Готового «потокового» элемента нет. Изображение — это `Arc<StaticImage>` (`warpui_core/src/image_cache.rs:393-414`, конструктора снаружи нет). Metal-рендерер кэширует текстуры **по указателю `Arc`** (`rendering/texture_cache.rs:49-78`): новый `Arc` на каждый кадр означает новую `MTLTexture` и `replaceRegion` (`metal/renderer.rs:461-503`), а старая текстура освобождается в `end_frame`, как только `Arc` отпущен (`texture_cache.rs:84-93`). Для MVP этого хватает: одна текстура в полёте, памяти не копится. Но это аллокация 4–10 МБ на кадр. Правильный путь — примитив «внешняя текстура со стабильным id», которая обновляется на месте (раздел 4.3).
6. **Цена кадра в Warp.** Любой `notify` перестраивает layout и paint всего окна и перерисовывает всю сцену (`presenter.rs:318-331`, `:333-380`). Это та же цена, что у терминала с потоковым выводом, который троттлится до 60/с (`app/src/terminal/view.rs:628`, `:3811`). Значит, лимит — тоже 60/с, и брать нужно только последний кадр.
7. **Физические устройства** — через scrcpy-server (Apache-2.0, jar около 717 КБ в v5.0.1), с нашим Rust-клиентом. adb-клиент из фазы A подходит почти без доработок: jar пушится через `exec:cat > …`, а сокеты scrcpy открываются сервисом `localabstract:scrcpy_<scid>` прямо через `AdbClient::open_service` (`adb/mod.rs:175`), без `adb forward` и без локальных портов. Декодировать стоит через **VideoToolbox** (`objc2-video-toolbox`, та же семья `objc2` 0.3, что уже в workspace). `openh264` уже есть в workspace (`Cargo.toml:233`), но только для интеграционных тестов, и это программный декодер. Протокол scrcpy «внутренний», клиент и сервер должны совпадать по версии точно (`develop.md`, Execution/Protocol), поэтому jar фиксируем по версии.
8. **Крейты и protoc.** `prost` 0.14 уже в workspace (`Cargo.toml:255-258`), `tonic` нет. Предлагается `tonic` 0.14 + `tonic-prost` и **закоммиченный сгенерированный код** из вендоренного `emulator_controller.proto`. Генерировать его фоновым скриптом через `protox` (компилятор protobuf на чистом Rust) и `tonic-prost-build::compile_fds`. Ни build.rs, ни protoc новому крейту не нужны, и он остаётся protoc-free, как остальные fork-крейты.
9. **План (7 PR):**
   - (1) крейт `warpium_emulator`: discovery, gRPC-клиент, фейковый сервер;
   - (2) панель «Устройства» с кадрами уже запущенного эмулятора;
   - (3) ввод и кнопки;
   - (4) встроенный запуск из тулбара и жизненный цикл;
   - (5) примитив потоковой текстуры и, по замерам, MMAP;
   - (6) физические устройства через scrcpy и VideoToolbox;
   - (7) несколько устройств, док, раскатка.

   Проверить в VM можно всё, кроме настоящего эмулятора и USB-устройства (раздел 7.4).

---

## 2. Вопрос 1: gRPC-эндпоинт эмулятора

### 2.1. Когда поднимается gRPC и как он защищён

Логика в `android-qemu2-glue/qemu-setup.cpp` (`emu-master-dev`):

| Запуск | Адрес | Порт | Авторизация | Источник |
|---|---|---|---|---|
| `emulator -avd X` (фаза A сегодня) | loopback (`127.0.0.1`, при неудаче `[::1]`) | первый свободный в `serial+3000 … serial+4000` | токен **и** JWT | `:386-396`, `:466-490`; `GrpcServices.cpp:239-260`, `:362-367` |
| `… -qt-hide-window -grpc-use-token -idle-grpc-timeout 300` (Studio) | loopback | так же | токен (+ JWT) | `EmulatorCommandBuilder.kt:64-68` |
| `… -grpc 8554` | **`[::]`, все интерфейсы** | ровно 8554 | **нет**, если не добавить `-grpc-use-token`/`-grpc-use-jwt`; эмулятор пишет warning `No gRPC protection active` | `:392-396`, `:466-469`; `GrpcServices.cpp:356-360` |
| `… -ports 5560,5561` без `-grpc` | gRPC не поднимается | — | — | `:627-632` |

Если авторизация включена, но TLS нет, сервер принудительно переходит на loopback (`GrpcServices.cpp:334-341`). Значит, Warpium **не должен** передавать `-grpc <port>`: порт и токен он берёт из файла обнаружения.

Статический токен даёт доступ ко всем методам `EmulatorController`, `UiController`, `SnapshotService` и incubating-сервисам: он выдаётся как issuer `android-studio` (`GrpcServices.cpp:343-344`), а allowlist для этого issuer открывает `/android.emulation.control.EmulatorController/.*` и остальные (`emulator_access.json`, блок `"iss": "android-studio"`). JWT нужен сторонним клиентам с ограниченными правами (`gradle-utp-emulator-control`). Warpium хватит токена.

### 2.2. Файл обнаружения

Эмулятор пишет `pid_<pid>.ini` со свойствами (`qemu-setup.cpp:376-384`, `:475`, `:488-489`, `:523-536`):

```
port.serial=5554           port.adb=5555
avd.name=Pixel_8_API_35    avd.id=…    avd.dir=/Users/…/.android/avd/Pixel_8_API_35.avd
emulator.version=…         emulator.build=…
cmdline=…                  (по нему Studio отличает встроенный запуск: содержит "-qt-hide-window", EmulatorId.kt:37-38)
grpc.port=8554             grpc.token=<base64, 64 случайных байта>
grpc.jwks=<dir>            grpc.jwk_active=<dir>/active.jwk
grpc.allowlist=…           (grpc.server_cert / grpc.ca_root — только при TLS-флагах)
```

Каталог (`RunningEmulatorCatalog.kt:597-617`, `REGISTRATION_DIRECTORY_RELATIVE_PATH = "avd/running"`, `:671`):

- macOS: `~/Library/Caches/TemporaryItems/avd/running/`;
- Linux: `$XDG_RUNTIME_DIR` → `/run/user/<uid>` → `~/.android` → `$TMP/android-<user>`, первый существующий, плюс `avd/running`;
- Windows: `%LOCALAPPDATA%/Temp/avd/running`.

Studio по имени файла проверяет, жив ли процесс, а запись без `grpc.port`, `avd.*` или `cmdline` отбрасывает (`RunningEmulatorCatalog.kt:414-494`).

Исправления к постановке:

- `~/.emulator_console_auth_token` — это токен **telnet-консоли** (порт `5554`, команда `auth`), к gRPC он отношения не имеет;
- `$ANDROID_SDK_HOME` / `$ANDROID_EMULATOR_HOME` на каталог discovery не влияют: у Studio путь фиксирован.

Для запущенного нами эмулятора файл находится по pid дочернего процесса из `launch_avd`. Для чужого (Studio, CLI) — по `avd.dir`/`avd.name` или по совпадению `port.serial` с серийником `emulator-5554` из трекера adb, так что сопоставление с `AndroidModel` бесплатное.

### 2.3. `streamScreenshot`: формат, размер, частота, задержка

- **Форматы** (proto `:1014-1027`):
  - `PNG` — дорого, только для снимков экрана;
  - `RGBA8888` — 4 байта на пиксель;
  - `RGB888` — 3 байта.

  Studio стримит `RGB888` и на CPU распаковывает его в `int[]` (`EmulatorView.kt:820`, `:1470`, `ImageConverter.unpackRgb888`). Нам выгоднее `RGBA8888`: на 33% больше байт, но текстура Warp — `RGBA8Unorm` (`metal/renderer.rs:469`, `wgpu/renderer/image.rs:267-269`), и кадр копируется в неё как есть.
- **Размер.** Запрошенные `width`/`height` эмулятор масштабирует с сохранением пропорций и никогда не превышает разрешение дисплея (proto `:120-137`, `:1036-1052`). Studio запрашивает размер по физическим пикселям своей панели (`EmulatorView.kt:797-826`) и пересоздаёт поток, только если размер или поворот изменились (`:811-814`). Кадр приходит уже повёрнутым (proto `:126-130`), а `ImageFormat.rotation` говорит, в каком квадранте (`:1032-1034`, `:1094-1111`). Если поворот не совпадает с ожидаемым, Studio пересоздаёт поток (`EmulatorView.kt:1442-1458`).
- **Порядок строк.** В proto написано «left to right and bottom up» (`:1074-1078`), но Studio кадр не переворачивает (`unpackRgb888` в `BufferedImage`). Похоже, комментарий устарел. В PR 2 это надо проверить на живом эмуляторе и так же настроить фейковый сервер.
- **Частота.** Цикл ждёт события фреймбуфера до 125 мс и шлёт кадр, только если он появился, изменились сенсоры (поворот) или это первый кадр (`EmulatorService.cpp:766-815`). Фактически это частота отрисовки гостя, до 60 к/с. Статичный экран — 0 к/с. Неактивный дисплей — один пустой кадр (`width == 0`), дальше тишина. Поле `seq` монотонно и с пропусками, по нему считаются потерянные кадры (proto `:1080-1085`). `timestampUs` — время кадра в эмуляторе, по нему можно измерять задержку до показа (`:1087-1091`).
- **Размер сообщения.** Кадр 1080×2400 в RGBA — 10,4 МБ, а `tonic` по умолчанию режет входящие сообщения на 4 МБ. Нужен `max_decoding_message_size(w*h*4 + запас)`. Studio делает то же самое: `maxDisplayPixels * 3 + 100` и переподключается, если не хватило (`EmulatorController.kt:260-261`, `:306-317`, `:1620-1627`).
- **MMAP/shm.** Клиент передаёт `transport { channel: MMAP, handle }`, где `handle` — `file:///…` (mmap-файл) или имя shm-региона, и сам владеет этим регионом (proto `:981-1001`). Эмулятор пишет пиксели прямо в регион, а в gRPC-сообщении остаются только метаданные (`EmulatorService.cpp:1032-1050`). На macOS это POSIX shm или mmap-файл. Синхронизации нет, так что «the mmap can result in tearing». Режим полезен как оптимизация (раздел 4.4), но для MVP не нужен.
- **Задержка.** Готовых цифр от Google нет. Цепочка такая: рендер гостя → readback и масштабирование в эмуляторе → loopback HTTP/2 → копия в текстуру → ближайший кадр Warp, обычно 1–2 кадра (16–33 мс) сверх рендера гостя. Мерить по `timestampUs` в PR 2.

### 2.4. Ввод, кнопки, поворот, снимок, звук

| Что | RPC | Детали |
|---|---|---|
| Мышь как один палец | `streamInputEvent(stream InputEvent{mouse_event})` | `x, y` в физических координатах дисплея, `buttons` 0/1/2 (proto `:721-740`). Studio шлёт именно мышь, касания — только в мульти-тач режиме (`EmulatorView.kt:1324-1356`) |
| Мульти-тач (щипок) | `InputEvent{touch_event}` | до 10 касаний, `identifier`, `pressure>0` при касании и `0` при отпускании, `expiration = NEVER_EXPIRE` требует обязательного `pressure=0` (proto `:621-683`). Studio эмулирует щипок вторым пальцем, зеркальным относительно центра (`EmulatorView.kt:1343-1344`) |
| Колесо и трекпад | `injectWheel(stream WheelEvent)` или `InputEvent{wheel_event}` | `dx`/`dy`, 120 = один щелчок (proto `:741-763`) |
| Клавиши | `sendKey(KeyboardEvent)` или `InputEvent{key_event}` | `key` — W3C-имя (`"Enter"`, `"Backspace"`, `"ArrowLeft"`, `"a"`), `eventType` — `keydown`/`keyup`/`keypress`; `text` — UTF-8 строка, надёжно только ASCII 32–127, для длинного или не-ASCII текста — clipboard API (proto `:765-852`) |
| Back / Home / Overview / Power | `sendKey{key:"GoBack"\|"GoHome"\|"AppSwitch"\|"Power"}` | proto `:828-835`; Studio: `EmulatorBackButtonAction` = `EmulatorPushButtonAction("GoBack")` |
| Громкость | `sendKey{key:"AudioVolumeUp"/"AudioVolumeDown"}` | Studio: `EmulatorVolumeUpButtonAction` |
| Поворот | `setPhysicalModel{target: ROTATION, value: [0, 0, angle]}` | angle ∈ {0, 90, −180, −90} (`EmulatorRotateAction.kt`) |
| Снимок экрана | `getScreenshot{format: PNG}` | Studio: `EmulatorScreenshotAction.kt` |
| Буфер обмена | `setClipboard` / `streamClipboard` | двусторонняя синхронизация, как `EmulatorClipboardSynchronizer.kt` |
| Фокус текстового поля, загрузка | `streamNotification` | `booted`, `textViewFocus`, `displayConfigurationsChanged`, `posture` (proto `:1358-1400`) |
| Выключение | `setVmState{SHUTDOWN}` | Studio при закрытии (`EmulatorController.kt:398-409`) |
| Звук | `streamAudio(AudioFormat)` | пакеты раз в 20–30 мс, может блокироваться при тишине (proto `:152-160`) |

Звук. Аудио-бэкенд эмулятора от окна не зависит, поэтому со скрытым Qt-окном эмулятор, по всей видимости, играет звук на хост сам. Это гипотеза, её надо проверить в PR 4. Если подтвердится, `streamAudio` не нужен. Если нет, в lock-файле уже есть `cpal` и `coreaudio-rs`.

### 2.5. Как устроен встроенный эмулятор Android Studio

- **Запуск.** `emulator -avd X -qt-hide-window -grpc-use-token -idle-grpc-timeout 300` (`EmulatorCommandBuilder.kt:62-68`). Официальная страница «Start the emulator from the command line» прямо пишет, что эти три аргумента нужны только для окна внутри Studio. С `-qt-hide-window` Qt-интерфейс создаётся, но окно скрыто, и через `UiController` можно открыть окно Extended Controls. `-no-window` убирает UI совсем и подходит для серверов (та же страница). Для паритета берём `-qt-hide-window`.
- **Подключение.** `RunningEmulatorCatalog` опрашивает каталог discovery (`:116`, `:291`). Для встроенного эмулятора создаётся `EmulatorController` с gRPC-каналом, `maxInboundMessageSize` и `TokenCallCredentials` (`EmulatorController.kt:306-349`).
- **Кадры:** `RGB888` по gRPC в размере панели (раздел 2.3).
- **Ввод:** один долгоживущий `streamInputEvent` (`EmulatorController.kt:591-621`), мышь в координатах дисплея с учётом поворота (`EmulatorView.kt:1264-1322`, формулы в разделе 4.5).
- **Жизнь:** keepalive `getVmState` каждые 2 минуты; при закрытии `setVmState(SHUTDOWN)`.
- **Версии.** Встроенный эмулятор появился в Android Studio 4.1, в октябре 2020 года (release notes 4.1: «Run the Android Emulator directly in Android Studio»), то есть в линейке эмулятора 30.x. Поля proto растут: `displayMode`, `foldedDisplay`, XR. Proto3 молча игнорирует неизвестные поля, так что старый эмулятор с новым клиентом совместим. Минимальную версию предлагается зафиксировать по `emulator.version` из discovery: всё, что старше 31.x, показывать сообщением «обновите эмулятор» без попытки стримить. Окончательный порог — по результату PR 2.

---

## 3. Вопрос 2: физические устройства через scrcpy

### 3.1. Протокол

По `doc/develop.md` scrcpy, разделы Execution и Protocol:

1. `adb push scrcpy-server /data/local/tmp/scrcpy-server.jar`.
2. Туннель. По умолчанию это `adb reverse localabstract:scrcpy_<SCID> tcp:27183`: компьютер слушает, устройство подключается. Запасной вариант — `forward`: устройство слушает. `SCID` — случайное 31-битное число.
3. `adb shell CLASSPATH=/data/local/tmp/scrcpy-server.jar app_process / com.genymobile.scrcpy.Server <версия клиента> scid=… key=value…`. **Версия должна совпадать точно**, иначе сервер падает. Протокол «internal: it may (and will) change at any time».
4. До трёх сокетов по порядку: video, audio, control. Каждый отключается (`video=false`, `audio=false`, `control=false`). В режиме forward на первом сокете приходит фиктивный байт, затем метаданные устройства (имя).
5. Video: `u32` codec id (`h264`/`h265`/`av1`, а также `vp8`/`vp9`), затем session packet на каждую сессию захвата, то есть на каждый поворот (12 байт: флаг, ширина, высота). Дальше media packets: 12-байтный заголовок (флаги config/key frame, PTS u61, размер u32) и сырой пакет `MediaCodec`.
6. Control — бинарные сообщения (`app/src/control_msg.h`): `INJECT_KEYCODE`, `INJECT_TEXT`, `INJECT_TOUCH_EVENT`, `INJECT_SCROLL_EVENT`, `BACK_OR_SCREEN_ON`, `SET_CLIPBOARD`, `SET_DISPLAY_POWER`, `ROTATE_DEVICE`, `UHID_*`, `RESIZE_DISPLAY`… Документацией служат тесты сериализации (`app/tests/test_control_msg_serialize.c`). Их байтовые векторы годятся для наших unit-тестов.

Требования scrcpy: Android 5.0 (API 21) и выше. Заявленная задержка — 35–70 мс, 30–120 к/с в зависимости от устройства (README).

### 3.2. Лицензия и доставка

scrcpy распространяется под Apache-2.0 (`LICENSE`; README: Copyright Genymobile, 2018–2026 Romain Vimont). Встраивать бинарный `scrcpy-server` можно при двух условиях:

- текст лицензии и NOTICE кладутся в бандл, а в «About → Licenses» появляется строка;
- явно указано, что используется немодифицированный сервер версии X.

Размер `scrcpy-server-v5.0.1` — 733 930 байт. Варианты:

| Вариант | Плюсы | Минусы |
|---|---|---|
| Класть jar в `Warpium.app/Contents/Resources` (рекомендуется) | работает офлайн, версия жёстко совпадает с клиентом, SHA256 проверяется при сборке | +0,7 МБ к бандлу; обновление scrcpy = PR с новым jar и правками клиента |
| Скачивать с GitHub Releases при первом использовании | бандл не растёт | сеть (у владельца GitHub медленный, около 40 КБ/с через VPN), проверка `SHA256SUMS.txt`, кэш |

Клиентский код scrcpy (C, тоже Apache-2.0) не берём. Портируем только формат пакетов, это пара сотен строк.

### 3.3. Что нужно от adb-клиента фазы A

`AdbClient` уже умеет `host:transport:<serial>` плюс любой сервис (`crates/warpium_android/src/adb/mod.rs:175-185`), `shell:` (`:188`) и бинарный `exec:` (`:197`). Для scrcpy понадобится немного:

- **Push без `sync:`.** `exec:cat > /data/local/tmp/warpium-scrcpy-<ver>.jar` и запись байтов в сокет. Это делает `adb exec-in`, работает на Android 5+. Протокол `sync:` (`SEND`/`DATA`/`DONE`) не нужен. Перед пушем — `shell:sha256sum` или проверка размера, чтобы не пушить при каждом запуске.
- **Туннель без портов.** Сервис adb `localabstract:<name>` подключается к абстрактному unix-сокету на устройстве напрямую, это тот же механизм, что `adb forward tcp:N localabstract:…`, только без локального порта. Сервер запускается с `tunnel_forward=true`, и три сокета открываются тремя вызовами `open_service(serial, "localabstract:scrcpy_<scid>")` по порядку. Так не нужны ни `adb reverse`, ни свободный TCP-порт, ни слушающий сокет в Warpium. Это работает и для Wi-Fi-устройств (`adb connect`).
- **Запуск сервера** — `exec:` или `shell:` с `CLASSPATH=… app_process / com.genymobile.scrcpy.Server 5.0.1 scid=… tunnel_forward=true audio=false max_size=<размер панели> max_fps=60 video_codec=h264 cleanup=true`. Поток процесса держим открытым, пока идёт сессия: закрытие = остановка сервера.

### 3.4. Декодирование H.264/H.265 на macOS

| Вариант | Что это | Аппаратно | Зависимости и лицензия | Вывод |
|---|---|---|---|---|
| **VideoToolbox** через `objc2-video-toolbox` + `objc2-core-media` + `objc2-core-video` (0.3) | системный декодер, `VTDecompressionSession` → `CVPixelBuffer` (BGRA или NV12, IOSurface-backed) | да, H.264 и HEVC; программный fallback внутри VT | только системные фреймворки; крейты той же семьи, что `objc2-metal`/`objc2-quartz-core` 0.3.2 (`Cargo.toml:240-246`); MIT/Apache | **рекомендуется** для macOS |
| `openh264` 0.8 | Cisco OpenH264, собирается из исходников (`openh264-sys2`) | нет | уже в lock (`Cargo.toml:233`; используется только `warpui_core` при `integration_tests`, `crates/warpui_core/Cargo.toml:13`, `:48`); BSD-2 | запасной вариант для Linux и тестов; только H.264, CPU выше |
| `ffmpeg-next` / `ffmpeg-sys` | libavcodec, с `videotoolbox` hwaccel | да, через hwaccel | системный или вендоренный FFmpeg (десятки МБ, LGPL/GPL, сборка) | нет: тяжело и лицензионно сложнее |

Схема для VT такая:

1. Из config-пакета (SPS/PPS) строится `CMVideoFormatDescription`.
2. Каждый media packet (Annex-B → AVCC: длины вместо стартовых кодов) идёт в `VTDecompressionSessionDecodeFrame`.
3. Callback отдаёт `CVPixelBuffer`.
4. Простой путь: просим `kCVPixelFormatType_32BGRA`, лочим базовый адрес и копируем в текстуру тем же путём, что кадры эмулятора. Нужна текстура BGRA8 или swizzle в шейдере.
5. Путь без копий: `CVMetalTextureCacheCreateTextureFromImage` даёт `MTLTexture` прямо из IOSurface. Это требует примитива внешней текстуры (раздел 4.3) и знания renderer-а о `CVMetalTexture`. Отложить до PR 7.

**Задержка и CPU (оценки, проверить в PR 6).** Аппаратный VT-декод 1080p — единицы миллисекунд на кадр и единицы процентов CPU. Основная задержка — энкодер на телефоне и USB (35–70 мс по данным scrcpy). openh264 на 1080p60 съест заметную долю ядра, поэтому по умолчанию его не используем.

Android Studio для физических устройств scrcpy не использует. У неё свой нативный агент `streaming/screen-sharing-agent` (пушится на устройство, кодирует через `MediaCodec`), а на хосте декодирует FFmpeg (`streaming/src/com/android/tools/idea/streaming/device/VideoDecoder.kt`, `avcodec_send_packet`). Повторять это смысла нет: scrcpy даёт тот же результат готовым, документированным и лицензионно чистым.

Эмулятор тоже adb-устройство, поэтому scrcpy работает и с ним. Это запасной путь, если gRPC недоступен (старый эмулятор, `-ports` без `-grpc`). Для эмулятора gRPC лучше: нет кодирования и декодирования, есть повороты, сенсоры и снимки.

---

## 4. Вопрос 3: рендеринг в warpui и ввод

### 4.1. Как сейчас рисуются изображения

- Элемент `Image` (`crates/warpui_core/src/elements/gui/image.rs`, 553 строки) берёт `Arc<StaticImage>` из `ImageCache` по `AssetSource` и в `paint` вызывает `ctx.scene.draw_image(rect, image, opacity, corner_radius)` (`image.rs:269-319`). Анимированные GIF перерисовываются через `repaint_after` (`image.rs:322-347`).
- `Scene::draw_image` кладёт в слой `scene::Image { bounds, asset: Arc<StaticImage>, opacity, corner_radius }` (`scene.rs:109-114`, `:589-610`).
- `StaticImage` — обёртка над `image::RgbaImage` с приватным полем (`image_cache.rs:393-414`). Создаётся только внутри `image_cache.rs` (`:318-355`, `:520-575`, `:688`), **публичного конструктора нет**.
- Metal: `Resources.texture_cache: TextureCache<MTLTexture>` (`metal/renderer.rs:147`). В `render_image_or_icon` вызывается `get_or_insert_by_asset`: если текстуры для этого `Arc` нет, создаётся `MTLTextureDescriptor` `RGBA8Unorm` нужного размера и заливается `replaceRegion` (`:461-503`). В конце кадра — `texture_cache.end_frame()` (`:840`).
- `TextureCache` ищет текстуру линейно, сравнивая `Weak::ptr_eq` (`texture_cache.rs:49-78`). Текстура удаляется, когда `strong_count() == 0` или ею не пользовались 10 кадров (`:84-93`).
- wgpu-рендерер (Linux и Windows, на macOS — под `cfg(wgpu)`) устроен так же: `get_or_insert_by_asset` → `create_texture` + `queue.write_texture` (`rendering/wgpu/renderer/image.rs:162`, `:267-269`).
- Максимальный размер текстуры на macOS — 8192 (`platform/mac/window.rs:1182-1185`). Дисплей телефона в него укладывается.

### 4.2. MVP: новый `StaticImage` на каждый кадр

Минимальная правка upstream — одна функция `StaticImage::from_rgba(width, height, Vec<u8>) -> Option<Self>` через `RgbaImage::from_vec`. С ней панель держит `Option<Arc<StaticImage>>` последнего кадра и рисует его обычным `draw_image`. Что происходит дальше:

- приходит кадр → новый `Arc` → в ближайшем кадре Warp создаётся новая `MTLTexture`, и в неё копируются байты (`replaceRegion`, CPU → unified memory);
- предыдущий `Arc` отпущен → в `end_frame` его текстура удаляется. Одновременно живут одна-две текстуры, ничего не копится;
- цена: аллокация текстуры 4–10 МБ и копирование на каждый кадр, плюс копия `Vec<u8>` из gRPC-буфера, если не использовать `bytes::Bytes` (раздел 5).

При 30 к/с и кадре 720×1600 это примерно 140 МБ/с memcpy и 30 аллокаций текстур в секунду. На Apple Silicon это терпимо, но расточительно. Поэтому MVP идёт с замером, и PR 5 заменяет этот путь.

### 4.3. Целевой путь: внешняя текстура со стабильным id

Новый примитив сцены, по смыслу аналог `Image`, но без asset-кэша:

```rust
// warpui_core (upstream-файлы, небольшая правка)
pub struct StreamTextureId(u64);                  // выдаёт App, живёт сколько живёт панель
pub struct StreamFrame { id, generation: u64, width, height, bytes: Bytes /* RGBA */ }
scene.draw_stream_texture(rect, id, corner_radius);   // в Layer.stream_textures
app.update_stream_texture(window_id, StreamFrame);    // кладёт кадр в «почтовый ящик» на 1 слот
```

Renderer держит `HashMap<StreamTextureId, (MTLTexture, generation)>`:

- если пришёл кадр с новой `generation`, он делает `replaceRegion` в **ту же** текстуру. Пересоздаёт её, только когда меняется размер (поворот, ресайз панели);
- если кадра нет, рисует старую текстуру без копирования. Это важно: Warp перерисовывает окно и из-за курсора, и из-за терминала, а лишней копии при этом не будет;
- когда панель закрыта, id освобождается и текстура удаляется.

Для wgpu то же самое через `queue.write_texture`. Объём правки — одна структура в `scene.rs`, ветки в двух renderer-ах, регистр в `App`. Примерно 300 строк в upstream-файлах: держать их отдельными модулями и вызывать одной строкой. Так же отделён остальной fork-код (WARPIUM.md, «Fork conventions»).

Позже, для VT без копий, тот же примитив принимает `StreamFrame::IoSurface(CVPixelBuffer)` и делает `CVMetalTextureCache`.

### 4.4. Темп кадров и бюджет

- **Почтовый ящик на 1 слот.** gRPC-поток перезаписывает последний кадр и шлёт `notify` только если предыдущий уже показан. Промежуточные кадры отбрасываются, очереди нет. Тот же принцип, что throttle 60/с у терминала (`app/src/terminal/view.rs:628`, `:3811`), и тот же механизм доставки из фонового потока, что у трекера устройств (`ctx.spawn_stream_local`, `app/src/warpium/android.rs:277-285`).
- **Цена `notify`.** Это `render_view` панели и полный layout и paint окна (`presenter.rs:318-331`, `:333-380`). Как у терминала, который печатает вывод. Отдельного троттлинга невидимых видов в Warp нет (см. bottom-panel.md, раздел 2.5), поэтому гасить поток на скрытой панели надо самим.
- **Пауза, когда панель не видна.** Если элемент не вызывал `paint` больше 500 мс (вкладка неактивна, док свёрнут, окно свернуто или перекрыто, и Warp не рисует), модель **отменяет** `streamScreenshot`. Эмулятор перестаёт делать readback. При следующем `paint` поток запрашивается заново, и первый кадр приходит сразу (`firstTime`, `EmulatorService.cpp:781`). Такой сторожок не требует хуков в upstream.

| Режим | Трафик | CPU Warpium (оценка, проверить в PR 2/5) | Память Warpium |
|---|---|---|---|
| Панели нет | 0 | 0 | 0, канала тоже нет |
| Панель скрыта | 0, поток отменён | 0, keepalive раз в 2 минуты | канал gRPC и последняя текстура (≤ 10 МБ); можно отпустить |
| Статичный экран | 0, кадров нет | ≈ 0 | 1 текстура + 1 буфер |
| Анимация 60 к/с, 720×1600 RGBA по gRPC | 276 МБ/с loopback | цель ≤ 15% одного P-ядра: HTTP/2 + memcpy + layout окна | 1 текстура + ≤ 2 буфера кадра (≈ 15–25 МБ) |
| То же через MMAP + внешнюю текстуру | ~100 Б на кадр | цель ≤ 5% | shm-регион (размер кадра) + 1 текстура |

Жёсткие правила, за которые отвечает code review:

- никакой истории кадров;
- никаких `Vec` на кадр вне почтового ящика;
- запрашивать размер = физические пиксели панели (`rect × scale_factor`), не больше дисплея. Пересоздавать поток с debounce 150 мс при ресайзе, как Studio (`EmulatorView.kt:809-814`);
- `max_fps` для scrcpy = 60.

Сам эмулятор (1,5–3 ГБ RAM гостя) — выбор пользователя. Warpium гарантирует, что скрытый эмулятор, который он запустил, не переживёт его больше чем на 5 минут (`-idle-grpc-timeout 300`).

### 4.5. Мышь → касание: масштаб, letterbox, поворот

Панель рисует кадр в прямоугольнике `R` (aspect-fit внутри панели, координаты в логических точках). События warpui приходят в логических точках (`LeftMouseDown/Dragged/Up { position }`, `crates/warpui_core/src/event.rs:108-123`; `ScrollWheel { delta, precise }`, `:102-107`), поэтому `scale_factor` в формулу не входит. Пусть `u = (p.x − R.x)/R.w`, `v = (p.y − R.y)/R.h`, `(W, H)` — дисплей в естественной ориентации, `q` — квадрант поворота из `ImageFormat.rotation`. Тогда, как в Studio (`EmulatorView.kt:1264-1322`):

| q | x устройства | y устройства |
|---|---|---|
| 0 (portrait) | `u·W` | `v·H` |
| 1 (landscape, 90°) | `(1−v)·W` | `u·H` |
| 2 (180°) | `(1−u)·W` | `(1−v)·H` |
| 3 (−90°) | `v·W` | `(1−u)·H` |

Точка вне дисплея игнорируется. Если это drag, Studio зажимает координаты в границы и отпускает палец (`:1316-1321`). Это чистая функция, её покрываем unit-тестами на все четыре квадранта и на letterbox. Для scrcpy координаты передаются вместе с размером кадра, который видит клиент (поле `screen_size` в `INJECT_TOUCH_EVENT`), так что поворот учитывает сервер.

Колесо: `precise` (трекпад) → `dy·k`, иначе `±120` за щелчок. Знак надо подобрать на устройстве: Studio инвертирует знак AWT (`EmulatorView.kt:1231`).

Мульти-тач: удерживать `alt` и тянуть — второй палец зеркально относительно центра, как в Studio.

### 4.6. Клавиатура и фокус

- В warpui есть только `KeyDown { keystroke, chars, details, is_composing }` и `TypedCharacters` (`event.rs:93-101`, `:186-193`), **события `KeyUp` нет**, как и сырого keycode (`Keystroke` = модификаторы + `key: String`, `keymap.rs:321-328`). Значит, каждую клавишу шлём как `keypress` (down + up). Удержание клавиш (автоповтор в игре, долгое нажатие) не поддерживается. Для IDE-сценария этого достаточно. Нужно ли добавлять `KeyUp` в warpui — решать позже, это правка платформенного слоя.
- Порядок разбора нажатия:
  1. Если `keystroke` — привязка Warpium (`cmd-*`, палитра, переключение панелей), её обрабатывает Warpium.
  2. Если это именованная клавиша (Enter, Backspace, Tab, Escape, стрелки, Home/End, PageUp/Down, Delete), отправляем `key` с W3C-именем. Таблица соответствий строится по `DomKeyNames.kt` из Studio.
  3. Если это печатаемый ASCII, отправляем `key`/`text`.
  4. Не-ASCII (кириллица, IME, `TypedCharacters`) — `setClipboard` + `ctrl+V` в эмуляторе, потому что proto прямо говорит, что произвольный UTF не дойдёт (proto `:840-852`).
- Фокус: клик по панели забирает фокус (`EventHandler::on_left_mouse_down`, `on_keydown`, `on_scroll_wheel`, `elements/gui/event_handler.rs:101-192`). Рамка фокуса — по `gui-ui-guidelines`. `Esc` отправляется в устройство, выход из панели — общими клавишами навигации Warpium.

---

## 5. Вопрос 4: крейты, gRPC и protoc

Что уже есть в workspace:

- `prost`/`prost-build`/`prost-reflect`/`prost-types` 0.14 (`Cargo.toml:255-258`);
- `hyper` 1.8 и `h2` 0.4 (через `reqwest`), `tokio` 1.50, `tower` 0.5 (`Cargo.toml:187`, `:306`, `:309`);
- `openh264` (`:233`);
- семья `objc2-*` 0.3 (`:240-246`).

`tonic` в lock-файле **нет**. protoc сейчас нужен только `crates/remote_server/build.rs:4-7` (`prost_build::compile_protos`), из-за него приложение и не собирается в контейнере (WARPIUM.md, «Fork conventions»).

**Варианты gRPC-клиента:**

| Вариант | За | Против |
|---|---|---|
| **`tonic` 0.14 + `tonic-prost`** (рекомендуется) | промышленный, server-streaming и client-streaming (`streamInputEvent`) из коробки, `max_decoding_message_size`, interceptor для `authorization`; совместим с prost 0.14 и hyper 1, которые уже в дереве; тот же крейт даёт **фейковый сервер** для тестов | новые крейты `tonic`, `tonic-prost` и пара транзитивных; нужен tokio runtime |
| Свой gRPC поверх `h2` | без новых зависимостей: unary и streaming — это HTTP/2 POST с 5-байтным префиксом и `grpc-status` в трейлерах | 300+ строк своего протокола, тестовый сервер тоже писать самим |

**Генерация кода без protoc.** `emulator_controller.proto` (Apache-2.0) вендорится в `crates/warpium_emulator/proto/` вместе с ревизией AOSP. Генерирует его скрипт `script/warpium/gen-emulator-proto` (fork-only), который запускает маленький бинарник:

1. `protox::compile` — разбор .proto на чистом Rust, well-known `google/protobuf/empty.proto` встроен;
2. `tonic_prost_build::configure().build_server(true).bytes(["."]).compile_fds(fds)`.

Результат — `src/proto/generated.rs` в git. Обычной сборке не нужны ни build.rs, ни protoc, ни protox, и `cargo test -p warpium_emulator` идёт в контейнере. Серверная часть сгенерированного кода нужна фейковому эмулятору и закрыта feature `test-support` (по образцу `warpium_android`, `crates/warpium_android/Cargo.toml`). `bytes(["."])` превращает `Image.image` в `bytes::Bytes` без лишней копии из HTTP/2-буфера.

**Runtime.** `warpium_android` работает на `std::thread` (`worker.rs`, `tracker.rs`). Эмулятор-клиент получает один поток с `tokio` current-thread runtime на подключённый эмулятор. Поток живёт, только пока открыта панель или идёт keepalive встроенного эмулятора, и умирает вместе с каналом.

**Декодирование видео** — раздел 3.4. `objc2-video-toolbox`, `objc2-core-media`, `objc2-core-video` 0.3 подключаются только под `cfg(target_os = "macos")` в отдельном крейте `warpium_mirror`. На других платформах — `openh264`, если понадобится.

---

## 6. Вопрос 5: UX

### 6.1. Где живёт экран устройства

| Место | За | Против |
|---|---|---|
| **Обычная панель Warp** (`IPaneType::WarpiumDevice`), по умолчанию split справа от активной (рекомендуется) | телефон портретный, справа ему место, как Running Devices в Studio (по умолчанию справа); вся механика панелей (split, resize, maximize, перенос во вкладку) бесплатна; шаблон уже есть — Logcat (`app/src/pane_group/pane/warpium_logcat_pane.rs:16-130`, `IPaneType::WarpiumLogcat` в `pane/mod.rs:153`, `LeafContents::WarpiumLogcat` в `app_state.rs:160`, открытие в `workspace/view/warpium_android.rs:36-90`) | в раскладке занимает место, пока открыта |
| Нижняя панель (пункт 14, `docs/warpium/research/bottom-panel.md`, PR #30) | естественное место для долгих процессов | низкая и широкая полоса плохо подходит портретному экрану; док ещё не смёржен |

Рекомендация: панель в раскладке. Как только появится док из пункта 14, «Move to panel» сработает и для неё, потому что док принимает любую `PaneContent`. Позже можно сделать боковой док. Отдельной логики не нужно, но для скрытой в доке панели обязательна пауза потока (раздел 4.4).

Как Logcat (`LogcatPaneManager::find_pane(window_id)`, `warpium_android.rs:50`), панель одна на окно, а устройства внутри — подвкладки, как в Running Devices. Восстановление сессии для панели не делаем, как для Logcat (`pane_group/mod.rs:2021`): после рестарта эмулятор, скорее всего, уже другой.

### 6.2. Тулбар и выбор устройства

Чип устройства в run-виджете (`app/src/warpium/project_toolbar.rs:291-293`, `ToggleDeviceMenu` / `SelectAndroidDevice` / `LaunchAndroidAvd`; пункты меню из `device_menu_items`, `app/src/warpium/android.rs:531`) получает:

- **AVD, не запущен** — «Запустить» (режим по настройке) и подменю «В панели Warpium / В отдельном окне». Настройка `warpium.android.emulator_launch = "embedded" | "window"`. По умолчанию `"window"`, пока флаг не раскатан, затем `"embedded"`;
- **эмулятор запущен** (наш или чужой, Studio/CLI) — «Показать в Warpium» (attach по discovery, раздел 2.2);
- **физическое устройство** — «Зеркалировать в Warpium» (scrcpy, PR 6).

Run (`run_android_app`) после установки показывает панель выбранного устройства, если она уже открыта или включена настройка `warpium.android.show_device_on_run`. Сама Run-логика ожидания загрузки (`AndroidEvent::AvdBooted`, `project_toolbar.rs:494-515`) не меняется: встроенный эмулятор — тот же adb-девайс.

Палитра: «Show Running Devices», «Rotate device left/right», «Take device screenshot», «Stop emulator».

### 6.3. Панель

- Сверху тонкая полоса: имя устройства или вкладки устройств, кнопки ⏻, 🔊+, 🔊−, ⟲, ⟳, ◁ (Back), ○ (Home), ▢ (Overview), 📷 (снимок в `~/Desktop/<avd>-<time>.png` + копия в буфер), меню «…» (Extended Controls через `UiController`, Stop, Snapshot). Иконки и цвета берутся из темы (`gui-ui-guidelines`).
- Кадр по центру, aspect-fit, без рамки-скина в MVP.
- Состояния: «Подключение…», «Загрузка…» (до `BootCompletedNotification` или `sys.boot_completed`), «Экран выключен» (пустой кадр), «Эмулятор завершился» с кнопками «Запустить снова» и «Закрыть», «Нужна версия эмулятора ≥ X».

### 6.4. Несколько устройств и завершение

- Каждое устройство — своя подвкладка и свой поток. Кадры показываются только у видимой подвкладки, остальные поставлены на паузу. Сколько устройств видно одновременно, столько и потоков.
- **Эмулятор завершился** (вышел процесс, discovery-файл исчез, или gRPC-поток вернул `UNAVAILABLE`, или adb-трекер убрал `emulator-NNNN`): поток и канал закрываются, текстура освобождается, в подвкладке остаётся состояние «Эмулятор завершился». Панель сама не закрывается, чтобы не прыгала раскладка. Если это была последняя подвкладка, кнопка «Закрыть панель» становится основной.
- **Закрытие панели или подвкладки встроенного эмулятора, который запустил Warpium**, спрашивает «Остановить эмулятор?» (`setVmState(SHUTDOWN)`), с галочкой «больше не спрашивать». У чужого эмулятора закрывается только просмотр.
- **Выход из Warpium** — встроенные эмуляторы получают `SHUTDOWN`. Если Warpium упал, срабатывает `-idle-grpc-timeout 300`. Эмуляторы в отдельном окне, как и сейчас, живут дальше (`avd.rs:143-144`).

---

## 7. Вопрос 6: план, риски, бюджет, тесты

### 7.1. Фазы (PR-размер)

Все пункты закрыты флагом `WarpiumEmbeddedEmulator` (скилл `add-feature-flag`) и общей настройкой `warpium.android`.

**PR 1. Крейт `warpium_emulator` (без UI).**

- Вендоренный proto, закоммиченный сгенерированный код, скрипт генерации (раздел 5).
- `discovery`: каталог по платформе, парсер `pid_*.ini`, проверка жив ли pid, сопоставление с серийником.
- `client`: канал `tonic`, Bearer-interceptor, `max_decoding_message_size`, `stream_screenshot(size, format) -> FrameStream`, `input_sender()` (`streamInputEvent`), `send_key`, `set_rotation`, `screenshot_png`, `keepalive`, `shutdown`.
- `mailbox`: почтовый ящик на один кадр.
- `geometry`: формулы раздела 4.5; `keys`: таблица Keystroke → W3C.
- `test_support::FakeEmulator`: tonic-сервер, синтетические кадры (градиент + номер кадра), запись принятых событий ввода и заголовка авторизации.
- Тесты: `cargo test -p warpium_emulator` в контейнере.

**PR 2. Панель «Устройства»: просмотр уже запущенного эмулятора.**

- `StaticImage::from_rgba` (одна функция в upstream).
- `IPaneType::WarpiumDevice` по шаблону Logcat.
- «Показать в Warpium» в меню устройства и в палитре.
- Пауза потока, когда панель не рисуется.
- Метрики в debug-логе: к/с, отброшено, задержка по `timestampUs`, время копирования.
- **Замер на Mac владельца** — главный результат PR 2, решающий, нужен ли PR 5 раньше PR 3. Здесь же проверить порядок строк и звук.

**PR 3. Ввод.** Мышь и касания, колесо, клавиатура с правилами раздела 4.6, кнопки панели (Back/Home/Overview/Power/громкость/поворот/снимок), фокус, мульти-тач с `alt`.

**PR 4. Встроенный запуск и жизненный цикл.**

- `launch_avd_embedded` (`-qt-hide-window -grpc-use-token -idle-grpc-timeout 300`, без `-grpc`) рядом с `launch_avd` (`avd.rs:145`).
- Discovery по pid ребёнка, keepalive, `SHUTDOWN` при закрытии и выходе.
- Состояния панели, настройка `emulator_launch`, интеграция с run-виджетом (раздел 6.2).
- WARPIUM.md.

**PR 5. Производительность** (порядок и объём — по замерам PR 2).

- Примитив внешней текстуры (раздел 4.3) в `warpui_core` + Metal + wgpu.
- Опционально MMAP-транспорт: shm-регион, которым владеет Warpium, кадр копируется прямо из него. Защита от tearing: двойной буфер не поддерживается, поэтому сравниваем `seq` до и после копирования.

**PR 6. Физические устройства.**

- Крейт `warpium_mirror`: формат scrcpy (session/media-пакеты, control-сообщения), push через `exec:`, сокеты через `localabstract:`.
- Jar в ресурсах бандла, проверка SHA256, NOTICE.
- VideoToolbox-декодер → BGRA → тот же путь кадра.
- Ввод через control-сокет.
- «Зеркалировать в Warpium» в меню устройства.

**PR 7. Полировка и раскатка.** Подвкладки нескольких устройств, синхронизация буфера обмена, VT без копий через IOSurface, интеграция с доком пункта 14 (пауза в свёрнутом доке), README (коротко), промоушен флага.

### 7.2. Риски

| Риск | Где | Митигация |
|---|---|---|
| Аллокация текстуры на каждый кадр в MVP | `texture_cache.rs:49-93`, `metal/renderer.rs:461-503` | замер в PR 2; PR 5 — стабильная текстура |
| Полная перестройка окна на каждый кадр | `presenter.rs:333-380` | не больше 60/с, почтовый ящик, пауза скрытой панели; цена как у терминала с выводом |
| Кадр больше лимита `tonic` в 4 МБ | — | `max_decoding_message_size` от размера дисплея; Studio делает так же (`EmulatorController.kt:260`) |
| Безопасность порта | `qemu-setup.cpp:392-396` | никогда не передавать `-grpc <port>`; только loopback и токен из discovery; токен не логировать (скилл `logging-and-error-reporting`) |
| Протокол scrcpy меняется между версиями | `develop.md`, Protocol | jar зафиксирован по версии и SHA256; формат пакетов покрыт тестами на векторах scrcpy; обновление — отдельный PR |
| Нет `KeyUp` в warpui | `event.rs:93-101` | `keypress` на нажатие; удержание не поддерживаем; при необходимости — отдельная правка платформенного слоя |
| Не-ASCII ввод | proto `:840-852` | через буфер обмена |
| Старые эмуляторы, другие поля proto | discovery `emulator.version` | порог версии и понятное сообщение; proto3 терпит неизвестные поля |
| Правки upstream-файлов (`image_cache.rs`, `scene.rs`, renderer-ы) | warpui | минимальные точки входа, новый код в отдельных модулях; список в WARPIUM.md «After merging upstream Warp» |
| Порядок строк кадра (bottom-up в proto) | proto `:1074-1078` | проверка в PR 2 на живом эмуляторе; фейковый сервер повторяет реальное поведение |
| Конфликт с доком пункта 14 | PR #30 | панель — обычная `PaneContent`, док её примет; пауза по отсутствию `paint` работает без знания о доке |

### 7.3. Бюджет производительности (приёмка)

Замеры — на Mac владельца (Apple Silicon), сборка `release`, эмулятор Pixel 1080×2400 API 35, панель около 400×880 pt (800×1760 px, запрос кадра 720×1600 после пропорционального вписывания):

1. Панель есть, экран статичный: CPU Warpium в пределах шума простоя (< 1%), кадров 0/с.
2. Панель скрыта, вкладка переключена: 0 кадров, через 500 мс поток отменён (проверка по debug-логу).
3. Прокрутка списка, около 60 к/с от гостя: CPU Warpium ≤ 15% одного P-ядра в MVP, ≤ 5% после PR 5; RSS Warpium выше простоя не больше чем на 30 МБ и без роста за 5 минут.
4. Задержка от `timestampUs` до показа: медиана ≤ 33 мс.
5. Закрытие панели: RSS вернулся к исходному (± шум), поток `warpium-emulator-*` завершился.
6. scrcpy 1080p60 H.264 с VT: CPU Warpium ≤ 10%, сквозная задержка на глаз как у scrcpy.

Если пункт 3 не проходит, PR 5 идёт до PR 3.

### 7.4. Тестовая стратегия

**Unit (`rust-unit-tests`, контейнер, `*_tests.rs` рядом):**

- discovery: разбор `pid_*.ini` (полный, без токена, без `grpc.port`, CRLF), выбор каталога по платформе и переменным, сопоставление с `emulator-5554`;
- геометрия: 4 квадранта × letterbox по горизонтали и вертикали × точки на границах; drag за край зажимается и отпускает палец;
- клавиши: таблица Keystroke → W3C, ASCII → `text`, не-ASCII → путь через буфер обмена, привязки Warpium не уходят в устройство;
- клиент против `FakeEmulator` в том же процессе: заголовок `authorization` (сервер отклоняет без него, по образцу `BasicTokenAuth`), кадр больше 4 МБ проходит, отмена потока останавливает сервер, порядок событий `streamInputEvent` сохраняется, keepalive;
- почтовый ящик: при медленном потребителе остаётся только последний кадр, память не растёт;
- scrcpy: разбор session/media-пакетов на байтовых фикстурах, сериализация control-сообщений по векторам `test_control_msg_serialize.c`, push через `exec:` и `localabstract:` против существующего фейкового adb-сервера (`crates/warpium_android/src/test_support.rs`);
- проверяется состояние, а не последовательность вызовов (AGENTS.md).

**Интеграция (`gui-integration-test`):** `FakeEmulator` в тестовом процессе и discovery-каталог, подменённый переменной `WARPIUM_EMULATOR_DISCOVERY_DIR` (только для тестов и отладки). Сценарии:

1. «Показать в Warpium» открывает панель, и на ней появляется кадр с номером ≥ N;
2. клик по центру → фейк получил касание с ожидаемыми координатами (для q = 0 и q = 1);
3. переключение вкладки → поток отменён;
4. остановка фейка → состояние «Эмулятор завершился».

**VM (warpium-mac MCP, `muhtar-test`).** Ставить CI-артефакт через `install_artifact`, не собирать в госте, потому что диск почти заполнен.

- **Можно проверить:**
  - сборку и запуск;
  - панель с `FakeEmulator`: синтетические кадры, скриншоты в светлой и тёмной теме, ресайз, letterbox;
  - маршрутизацию ввода (фейк пишет полученные события в лог, читаем через `app_logs`);
  - паузу на скрытой панели, состояния завершения и ошибки авторизации;
  - декодирование H.264-фикстуры через VideoToolbox (в госте VT, скорее всего, уйдёт в программный декодер, поэтому замер производительности там не показателен).
- **Нельзя проверить:**
  - **настоящий эмулятор**: он использует Hypervisor.framework, а Virtualization.framework даёт вложенную виртуализацию только Linux-гостям (`VZGenericPlatformConfiguration.isNestedVirtualizationEnabled`, macOS 15+, M3+), не macOS-гостю;
  - USB-устройство;
  - реальные цифры CPU и задержки;
  - звук.

  Это чек-лист для ручной проверки владельцем на его Mac после PR 2, 4 и 6: пункты раздела 7.3 плюс «запустить → Run → тап/текст/поворот/снимок → закрыть → эмулятор остановлен».

---

## 8. Открытые вопросы владельцу

1. Режим запуска по умолчанию после раскатки: встроенный (`-qt-hide-window`, рекомендуется) или как сейчас, в отдельном окне? Нужна ли настройка на каждый AVD?
2. Jar scrcpy (около 0,7 МБ) класть в бандл (рекомендуется) или скачивать при первом использовании?
3. Где по умолчанию открывать панель: справа (рекомендуется) или сразу в нижнем доке пункта 14?
4. Нужен ли звук устройства в Warpium, или достаточно того, что эмулятор выводит его сам (проверим в PR 4)?
5. Спрашивать ли «Остановить эмулятор?» при закрытии панели, или останавливать молча (как Studio при закрытии вкладки устройства)?
6. Порядок после PR 4: сначала физические устройства (PR 6) или производительность (PR 5), если замеры PR 2 укладываются в бюджет?
