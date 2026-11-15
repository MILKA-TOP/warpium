//! Warpium's Logcat pane: a live, filterable log of the selected Android device.
//!
//! The stream is read and parsed on a background thread (`warpium_android::logcat`) and arrives
//! here in batches of at most 20 per second. Rows live in a bounded [`LogBuffer`] that keeps the
//! filtered row list current incrementally, and the list only lays out the visible rows, so the
//! cost of a frame doesn't grow with the log. Stack-trace frames open their source through the
//! code index.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use code_index::SearchScope;
use futures::channel::oneshot;
use warp_core::r#async::debounce;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_util::path::LineAndColumnArg;
use warpium_android::logcat::{
    LogBuffer, LogEntry, LogFilter, LogLevel, LogTime, LogcatEvent, LogcatOptions, Row, TagFilter,
    TextFilter, spawn_logcat, spawn_process_poller,
};
use warpium_android::stack::{StackFrame, choose_source_file, find_source_file, parse_frame};
use warpium_android::{AdbClient, WorkerHandle};
use warpui::r#async::Timer;
use warpui::clipboard::ClipboardContent;
use warpui::elements::{
    Border, ChildAnchor, ChildView, Clipped, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, DispatchEventResult, Empty, EventHandler, Expanded, Fill, Flex,
    MouseStateHandle, OffsetPositioning, Padding, ParentAnchor, ParentElement, ParentOffsetBounds,
    Radius, ScrollStateHandle, Scrollable, ScrollableElement, ScrollbarWidth, Shrinkable, Stack,
    Text, UniformList, UniformListState,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::FixedBinding;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::button::Button;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle, WeakViewHandle, WindowId,
};

use super::android::{AdbStatus, AndroidEvent, AndroidModel, device_menu_items};
use super::code_index::CodeIndexModel;
use crate::editor::{
    EditorOptions, EditorView, Event as EditorEvent, PropagateHorizontalNavigationKeys, TextOptions,
};
use crate::menu::{Event as MenuEvent, Menu, MenuItem, MenuItemFields};
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::pane::view::{self, HeaderContent, StandardHeader, StandardHeaderOptions};
use crate::pane_group::{BackingView, PaneConfiguration, PaneEvent};
use crate::ui_components::buttons::icon_button;
use crate::util::bindings::CustomAction;
use crate::view_components::DismissibleToast;
use crate::workspace::{PaneViewLocator, ToastStack, WorkspaceAction};

const HEADER_TEXT: &str = "Logcat";
/// Entries kept; older ones are dropped.
const BUFFER_CAPACITY: usize = 200_000;
const FILTER_DEBOUNCE: Duration = Duration::from_millis(150);
/// How often the tracked app's processes are polled, as a safety net for ActivityManager's logs.
const PROCESS_POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Delay before reconnecting a stream that ended while its device stayed online.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const TOOLBAR_FONT_SIZE: f32 = 12.;
const CONTROL_HEIGHT: f32 = 24.;
const MENU_WIDTH: f32 = 280.;
/// Monospace character widths of the metadata columns.
const TIME_CHARS: f32 = 19.;
const PID_CHARS: f32 = 12.;
const TAG_CHARS: f32 = 23.;
/// Approximate advance of a monospace character relative to the font size.
const MONO_CHAR_WIDTH: f32 = 0.62;

pub(crate) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings([FixedBinding::custom(
        CustomAction::Copy,
        LogcatAction::CopySelection,
        "Copy",
        id!(LogcatView::ui_name()),
    )]);
}

/// Tracks the Logcat pane of each window, so opening Logcat focuses the existing one.
#[derive(Default)]
pub(crate) struct LogcatPaneManager {
    panes: HashMap<WindowId, PaneViewLocator>,
}

impl LogcatPaneManager {
    pub(crate) fn find_pane(&self, window_id: WindowId) -> Option<PaneViewLocator> {
        self.panes.get(&window_id).copied()
    }

    pub(crate) fn register_pane(&mut self, window_id: WindowId, locator: PaneViewLocator) {
        self.panes.insert(window_id, locator);
    }

    pub(crate) fn deregister_pane(&mut self, window_id: &WindowId) {
        self.panes.remove(window_id);
    }
}

impl Entity for LogcatPaneManager {
    type Event = ();
}

impl SingletonEntity for LogcatPaneManager {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LogcatViewEvent {
    Pane(PaneEvent),
}

/// A menu the pane can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogcatMenu {
    Devices,
    Levels,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LogcatAction {
    ToggleMenu(LogcatMenu),
    SelectDevice(String),
    LaunchAvd(String),
    SetLevel(LogLevel),
    ToggleRegex,
    TogglePause,
    Clear,
    ScrollToEnd,
    /// The user scrolled the list (positive is towards older rows).
    ScrolledBy(f32),
    /// The user pressed the mouse in the list (a click or a scrollbar drag).
    StopFollowing,
    SelectRow(u64),
    CopySelection,
    /// Open the source of the stack frame on this log line.
    OpenFrame(String),
    FocusEditor(FilterField),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterField {
    Package,
    Tag,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LogcatOverflowAction {
    CopyVisible,
    Clear,
}

/// The state of the log stream.
#[derive(Clone, Debug, PartialEq, Eq)]
enum StreamState {
    /// No device to read from.
    Idle,
    Streaming,
    /// The stream ended (with the reason, if it failed); it restarts when the device is back.
    Ended(Option<String>),
}

#[derive(Default)]
struct MouseStates {
    device_chip: MouseStateHandle,
    level_chip: MouseStateHandle,
    regex: MouseStateHandle,
    pause: MouseStateHandle,
    clear: MouseStateHandle,
    scroll_to_end: MouseStateHandle,
}

pub(crate) struct LogcatView {
    buffer: LogBuffer,
    /// The device the stream reads from.
    serial: Option<String>,
    stream: Option<WorkerHandle>,
    poller: Option<WorkerHandle>,
    /// Bumped on every (re)start, so batches from an older stream are dropped.
    stream_generation: u64,
    stream_state: StreamState,
    paused: bool,
    /// Entries received while paused, applied on resume.
    paused_backlog: Vec<LogEntry>,
    /// Keep the newest row in view.
    follow: bool,
    /// After reconnecting to the same device, entries up to this time are already shown (the new
    /// stream starts with the device's backlog), so they are skipped.
    resume_after: Option<LogTime>,
    /// The rows the list showed at its last layout.
    visible: Range<usize>,
    visible_tx: async_channel::Sender<Range<usize>>,
    /// Sequence number of the selected entry.
    selected: Option<u64>,
    level: LogLevel,
    regex: bool,
    filter_error: Option<String>,
    package_editor: ViewHandle<EditorView>,
    tag_editor: ViewHandle<EditorView>,
    text_editor: ViewHandle<EditorView>,
    filter_change_tx: async_channel::Sender<()>,
    menu: ViewHandle<Menu<LogcatAction>>,
    open_menu: Option<LogcatMenu>,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
    handle: WeakViewHandle<Self>,
    /// The project the pane was opened for, searched for stack-frame sources.
    project_root: Option<PathBuf>,
    pane_configuration: ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,
    mouse_states: MouseStates,
}

impl LogcatView {
    /// A pane for the selected device. `package` (the project's app) pre-fills the package filter.
    pub(crate) fn new(
        project_root: Option<PathBuf>,
        package: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let pane_configuration = ctx.add_model(|_| PaneConfiguration::new(HEADER_TEXT));
        let package_editor = Self::create_filter_editor("Package (all processes)", ctx);
        let tag_editor = Self::create_filter_editor("Tags (MyTag, -Noisy)", ctx);
        let text_editor = Self::create_filter_editor("Search", ctx);
        if let Some(package) = &package {
            package_editor.update(ctx, |editor, ctx| editor.set_buffer_text(package, ctx));
        }
        for editor in [&package_editor, &tag_editor, &text_editor] {
            ctx.subscribe_to_view(editor, |me, _, event, _| match event {
                EditorEvent::Edited(_)
                | EditorEvent::BufferReplaced
                | EditorEvent::BufferReinitialized => {
                    let _ = me.filter_change_tx.try_send(());
                }
                _ => {}
            });
        }
        let (filter_change_tx, filter_change_rx) = async_channel::unbounded();
        ctx.spawn_stream_local(
            debounce(FILTER_DEBOUNCE, filter_change_rx),
            |me, _, ctx| me.apply_filters(ctx),
            |_, _| {},
        );

        let menu = ctx.add_typed_action_view(|_| {
            Menu::new()
                .with_width(MENU_WIDTH)
                .with_drop_shadow()
                .prevent_interaction_with_other_elements()
        });
        ctx.subscribe_to_view(&menu, |me, _, event, ctx| match event {
            MenuEvent::Close { .. } => {
                me.open_menu = None;
                ctx.notify();
            }
            MenuEvent::ItemSelected | MenuEvent::ItemHovered => {}
        });

        ctx.subscribe_to_model(
            &AndroidModel::handle(ctx),
            |me, _, event, ctx| match event {
                AndroidEvent::DevicesChanged | AndroidEvent::SelectionChanged => {
                    me.sync_stream(ctx);
                    if me.open_menu == Some(LogcatMenu::Devices) {
                        me.refresh_menu(ctx);
                    }
                    ctx.notify();
                }
                AndroidEvent::AppLaunched(package) => {
                    if me.buffer.package() != Some(package.as_str()) {
                        me.set_package(package.clone(), ctx);
                    }
                }
                AndroidEvent::AvdsChanged
                | AndroidEvent::AvdBooted
                | AndroidEvent::AvdLaunchFailed => {
                    if me.open_menu == Some(LogcatMenu::Devices) {
                        me.refresh_menu(ctx);
                    }
                    ctx.notify();
                }
            },
        );

        let (visible_tx, visible_rx) = async_channel::unbounded();
        ctx.spawn_stream_local(
            visible_rx,
            |me, visible: Range<usize>, _| me.visible = visible,
            |_, _| {},
        );

        let mut buffer = LogBuffer::new(BUFFER_CAPACITY);
        buffer.set_package(package);
        let mut view = Self {
            buffer,
            serial: None,
            stream: None,
            poller: None,
            stream_generation: 0,
            stream_state: StreamState::Idle,
            paused: false,
            paused_backlog: Vec::new(),
            follow: true,
            resume_after: None,
            visible: 0..0,
            visible_tx,
            selected: None,
            level: LogLevel::Verbose,
            regex: false,
            filter_error: None,
            package_editor,
            tag_editor,
            text_editor,
            filter_change_tx,
            menu,
            open_menu: None,
            list_state: UniformListState::new(),
            scroll_state: ScrollStateHandle::default(),
            handle: ctx.handle(),
            project_root,
            pane_configuration,
            focus_handle: None,
            mouse_states: MouseStates::default(),
        };
        view.sync_stream(ctx);
        view
    }

    fn create_filter_editor(
        placeholder: &'static str,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<EditorView> {
        ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = EditorOptions {
                text: TextOptions::ui_text(Some(TOOLBAR_FONT_SIZE), appearance),
                select_all_on_focus: true,
                clear_selections_on_blur: true,
                propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys::Always,
                single_line: true,
                autogrow: false,
                soft_wrap: false,
                ..Default::default()
            };
            let mut editor = EditorView::new(options, ctx);
            editor.set_placeholder_text(placeholder, ctx);
            editor
        })
    }

    pub(crate) fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    pub(crate) fn focus(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
    }

    /// Sets the package filter, as after running an app.
    pub(crate) fn set_package(&mut self, package: String, ctx: &mut ViewContext<Self>) {
        self.package_editor
            .update(ctx, |editor, ctx| editor.set_buffer_text(&package, ctx));
        self.apply_filters(ctx);
    }

    pub(crate) fn show_menu(&mut self, menu: LogcatMenu, ctx: &mut ViewContext<Self>) {
        if self.open_menu != Some(menu) {
            self.toggle_menu(menu, ctx);
        }
    }

    /// Starts, restarts or stops the stream to follow the selected device.
    fn sync_stream(&mut self, ctx: &mut ViewContext<Self>) {
        let android = AndroidModel::as_ref(ctx);
        let client = android.client().cloned();
        let online_serial = android
            .selected_device()
            .filter(|device| device.entry.state.is_online())
            .map(|device| device.serial().to_owned());
        let selected_serial = android.selected_serial().map(str::to_owned);
        let device_changed = selected_serial != self.serial;
        match (online_serial, client) {
            (Some(serial), Some(client)) => {
                let running = self.stream_state == StreamState::Streaming;
                if device_changed || !running {
                    self.start_stream(client, serial, ctx);
                }
            }
            (None, _) | (_, None) => {
                if device_changed {
                    self.buffer.reset();
                    self.selected = None;
                }
                self.stop_stream();
                self.serial = selected_serial;
                self.stream_state = if self.serial.is_some() {
                    StreamState::Ended(None)
                } else {
                    StreamState::Idle
                };
            }
        }
        self.update_title(ctx);
    }

    fn stop_stream(&mut self) {
        self.stream = None;
        self.poller = None;
        self.stream_generation += 1;
    }

    fn start_stream(&mut self, client: AdbClient, serial: String, ctx: &mut ViewContext<Self>) {
        self.stop_stream();
        if self.serial.as_deref() == Some(serial.as_str()) {
            // The same device again (it reconnected or rebooted): keep what is shown, and skip
            // the part of the new stream's backlog that is already there.
            self.resume_after = self.buffer.last_time();
        } else {
            self.buffer.reset();
            self.paused_backlog.clear();
            self.selected = None;
            self.follow = true;
            self.resume_after = None;
        }
        self.serial = Some(serial.clone());
        let generation = self.stream_generation;
        let (sender, receiver) = async_channel::unbounded();
        match spawn_logcat(
            client.clone(),
            serial,
            LogcatOptions::default(),
            move |event| {
                let _ = sender.try_send(event);
            },
        ) {
            Ok(stream) => self.stream = Some(stream),
            Err(err) => {
                log::error!("[Logcat] Couldn't start the logcat thread: {err}");
                self.stream_state = StreamState::Ended(Some(err.to_string()));
                return;
            }
        }
        self.stream_state = StreamState::Streaming;
        ctx.spawn_stream_local(
            receiver,
            move |me, event, ctx| {
                if me.stream_generation == generation {
                    me.on_logcat_event(event, ctx);
                }
            },
            |_, _| {},
        );
        self.restart_poller(Some(client), ctx);
    }

    /// Polls the package filter's processes on the streamed device, if there is a package.
    fn restart_poller(&mut self, client: Option<AdbClient>, ctx: &mut ViewContext<Self>) {
        self.poller = None;
        let client = client.or_else(|| AndroidModel::as_ref(ctx).client().cloned());
        let (Some(client), Some(serial), Some(package)) = (
            client,
            self.serial.clone(),
            self.buffer.package().map(str::to_owned),
        ) else {
            return;
        };
        if self.stream_state != StreamState::Streaming {
            return;
        }
        let generation = self.stream_generation;
        let (sender, receiver) = async_channel::unbounded();
        match spawn_process_poller(
            client,
            serial,
            package.clone(),
            PROCESS_POLL_INTERVAL,
            move |processes| {
                let _ = sender.try_send(processes);
            },
        ) {
            Ok(poller) => self.poller = Some(poller),
            Err(err) => {
                log::warn!("[Logcat] Couldn't start polling processes: {err}");
                return;
            }
        }
        ctx.spawn_stream_local(
            receiver,
            move |me, processes: Vec<(i32, String)>, ctx| {
                if me.stream_generation != generation
                    || me.buffer.package() != Some(package.as_str())
                {
                    return;
                }
                if me.buffer.update_package_processes(&processes) {
                    me.after_rows_rebuilt();
                    ctx.notify();
                }
            },
            |_, _| {},
        );
    }

    fn on_logcat_event(&mut self, event: LogcatEvent, ctx: &mut ViewContext<Self>) {
        match event {
            LogcatEvent::Entries(mut entries) => {
                if let Some(after) = self.resume_after {
                    let skip = entries
                        .iter()
                        .take_while(|entry| entry.time <= after)
                        .count();
                    if skip < entries.len() {
                        self.resume_after = None;
                    }
                    entries.drain(..skip);
                    if entries.is_empty() {
                        return;
                    }
                }
                if self.paused {
                    self.paused_backlog.extend(entries);
                    let overflow = self.paused_backlog.len().saturating_sub(BUFFER_CAPACITY);
                    self.paused_backlog.drain(..overflow);
                    return;
                }
                self.push_entries(entries);
                ctx.notify();
            }
            LogcatEvent::Ended(reason) => {
                if let Some(reason) = &reason {
                    log::info!("[Logcat] The log stream ended: {reason}");
                }
                self.stream_state = StreamState::Ended(reason);
                self.poller = None;
                let generation = self.stream_generation;
                ctx.spawn(
                    async move { Timer::after(RECONNECT_DELAY).await },
                    move |me, _, ctx| {
                        if me.stream_generation == generation {
                            me.sync_stream(ctx);
                            ctx.notify();
                        }
                    },
                );
                ctx.notify();
            }
        }
    }

    fn push_entries(&mut self, entries: Vec<LogEntry>) {
        let outcome = self.buffer.push_batch(entries);
        if outcome.rebuilt {
            self.after_rows_rebuilt();
        } else if !self.follow && outcome.evicted_rows > 0 {
            self.list_state
                .add_scroll_top(-(outcome.evicted_rows as f32));
        }
        if self
            .selected
            .is_some_and(|seq| self.buffer.row_index_of(seq).is_none())
        {
            self.selected = None;
        }
    }

    /// Keeps the selection in view after the rows were rebuilt (filter or pid changes).
    fn after_rows_rebuilt(&mut self) {
        if self.follow {
            return;
        }
        match self.selected.and_then(|seq| self.buffer.row_index_of(seq)) {
            Some(index) => self.list_state.scroll_to(index),
            None => self.follow = true,
        }
    }

    fn apply_filters(&mut self, ctx: &mut ViewContext<Self>) {
        let package = self
            .package_editor
            .as_ref(ctx)
            .buffer_text(ctx)
            .trim()
            .to_owned();
        let tags = self.tag_editor.as_ref(ctx).buffer_text(ctx);
        let text = self.text_editor.as_ref(ctx).buffer_text(ctx);
        // An invalid regex keeps the previous text filter, and the other filters still apply.
        let text_filter = match TextFilter::parse(&text, self.regex) {
            Ok(filter) => {
                self.filter_error = None;
                filter
            }
            Err(err) => {
                self.filter_error = Some(format!("Invalid regex: {err}"));
                self.buffer.filter().text.clone()
            }
        };
        let package = (!package.is_empty()).then_some(package);
        let package_changed = self.buffer.package() != package.as_deref();
        self.buffer.set_package(package);
        self.buffer.set_filter(LogFilter {
            min_level: self.level,
            tags: TagFilter::parse(&tags),
            text: text_filter,
        });
        if package_changed {
            self.restart_poller(None, ctx);
        }
        self.after_rows_rebuilt();
        ctx.notify();
    }

    fn update_title(&self, ctx: &mut ViewContext<Self>) {
        let title = match self.device_label(ctx) {
            Some(device) => format!("{HEADER_TEXT} · {device}"),
            None => HEADER_TEXT.to_owned(),
        };
        self.pane_configuration.update(ctx, |configuration, ctx| {
            configuration.set_title(title, ctx)
        });
    }

    fn device_label(&self, ctx: &AppContext) -> Option<String> {
        let serial = self.serial.as_deref()?;
        let android = AndroidModel::as_ref(ctx);
        Some(
            android
                .devices()
                .iter()
                .find(|device| device.serial() == serial)
                .map(|device| device.display_name())
                .unwrap_or_else(|| serial.to_owned()),
        )
    }

    fn toggle_menu(&mut self, menu: LogcatMenu, ctx: &mut ViewContext<Self>) {
        if self.open_menu == Some(menu) {
            self.open_menu = None;
            ctx.notify();
            return;
        }
        if menu == LogcatMenu::Devices {
            let project_root = self.project_root.clone();
            AndroidModel::handle(ctx).update(ctx, |android, ctx| {
                android.ensure_started(project_root.as_deref(), ctx);
                android.refresh_avds(ctx);
            });
        }
        self.open_menu = Some(menu);
        self.refresh_menu(ctx);
        ctx.focus(&self.menu);
        ctx.notify();
    }

    fn refresh_menu(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(menu) = self.open_menu else {
            return;
        };
        let items = match menu {
            LogcatMenu::Devices => device_menu_items(
                AndroidModel::as_ref(ctx),
                LogcatAction::SelectDevice,
                LogcatAction::LaunchAvd,
            ),
            LogcatMenu::Levels => LogLevel::ALL
                .iter()
                .map(|level| {
                    MenuItem::Item(
                        MenuItemFields::new(level.name())
                            .with_on_select_action(LogcatAction::SetLevel(*level)),
                    )
                })
                .collect(),
        };
        let selected = match menu {
            LogcatMenu::Devices => self.serial.clone().map(LogcatAction::SelectDevice),
            LogcatMenu::Levels => Some(LogcatAction::SetLevel(self.level)),
        };
        self.menu.update(ctx, |menu_view, ctx| {
            menu_view.set_items(items, ctx);
            if let Some(selected) = &selected {
                menu_view.set_selected_by_action(selected, ctx);
            }
        });
    }

    /// The selected entry as `threadtime` text.
    fn selection_text(&self) -> Option<String> {
        let index = self.buffer.row_index_of(self.selected?)?;
        let entry = self.buffer.row(index)?.entry;
        Some(format_entry(entry))
    }

    /// Every filtered row as `threadtime` text.
    fn visible_text(&self) -> String {
        let mut text = String::new();
        let mut last_seq = None;
        for row in self.buffer.rows(0..self.buffer.row_count()) {
            if last_seq != Some(row.seq) {
                text.push_str(&format_entry(row.entry));
                text.push('\n');
                last_seq = Some(row.seq);
            }
        }
        text
    }

    fn open_frame(&mut self, line: &str, ctx: &mut ViewContext<Self>) {
        let Some(frame) = parse_frame(line) else {
            return;
        };
        let index = ctx
            .has_singleton_model::<CodeIndexModel>()
            .then(|| CodeIndexModel::as_ref(ctx).handle_for_search())
            .flatten();
        let mut roots: Vec<PathBuf> = self.project_root.iter().cloned().collect();
        if let Some(index) = &index
            && !roots.contains(&index.root)
        {
            roots.push(index.root.clone());
        }
        let (sender, receiver) = oneshot::channel();
        let thread_frame = frame.clone();
        let spawned = std::thread::Builder::new()
            .name("logcat-frame-source".to_owned())
            .spawn(move || {
                let class_files = index
                    .map(|index| class_files_for_frame(&thread_frame, &index.index.read()))
                    .unwrap_or_default();
                let path = choose_source_file(&thread_frame, &class_files).or_else(|| {
                    roots
                        .iter()
                        .find_map(|root| find_source_file(root, &thread_frame))
                });
                let _ = sender.send(path);
            });
        if let Err(err) = spawned {
            log::warn!("[Logcat] Couldn't start the source lookup: {err}");
            return;
        }
        ctx.spawn(
            async move { receiver.await.ok().flatten() },
            move |_, path, ctx| match path {
                Some(full_path) => {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::OpenFileInNewTab {
                        full_path,
                        line_and_column: Some(LineAndColumnArg {
                            line_num: frame.line.unwrap_or(1).max(1) as usize,
                            column_num: None,
                        }),
                    })
                }
                None => {
                    let file = if frame.file.is_empty() {
                        frame.class_name.clone()
                    } else {
                        frame.file.clone()
                    };
                    let window_id = ctx.window_id();
                    ToastStack::handle(ctx).update(ctx, |stack, ctx| {
                        stack.add_ephemeral_toast(
                            DismissibleToast::error(format!("Couldn't find {file} in the project")),
                            window_id,
                            ctx,
                        );
                    });
                }
            },
        );
    }

    fn status_message(&self, ctx: &AppContext) -> Option<(String, bool)> {
        if let Some(error) = &self.filter_error {
            return Some((error.clone(), true));
        }
        let android = AndroidModel::as_ref(ctx);
        if let AdbStatus::Unavailable(message) = android.status() {
            return Some((format!("Can't reach the adb server: {message}"), true));
        }
        if self.paused {
            return Some((
                format!("Paused · {} new entries waiting", self.paused_backlog.len()),
                false,
            ));
        }
        match &self.stream_state {
            StreamState::Idle => {
                let message = match android.status() {
                    AdbStatus::NotStarted | AdbStatus::Connecting => "Connecting to adb…",
                    AdbStatus::Connected | AdbStatus::Unavailable(_) => {
                        "No device. Connect one or launch an emulator from the device menu."
                    }
                };
                return Some((message.to_owned(), false));
            }
            StreamState::Ended(reason) => {
                let device = self.device_label(ctx).unwrap_or_default();
                let state = android
                    .selected_device()
                    .map(|device| device.entry.state.label().to_owned())
                    .unwrap_or_else(|| "disconnected".to_owned());
                let online = android
                    .selected_device()
                    .is_some_and(|device| device.entry.state.is_online());
                let message = match reason {
                    Some(reason) if online => {
                        format!("{device}: logcat stopped ({reason}); reconnecting…")
                    }
                    None if online => format!("{device}: logcat stopped; reconnecting…"),
                    Some(_) | None => {
                        format!("{device} is {state}; logcat resumes when it is back")
                    }
                };
                return Some((message, false));
            }
            StreamState::Streaming => {}
        }
        if let Some(package) = self.buffer.package() {
            if !self.buffer.package_seen() {
                return Some((format!("Waiting for {package} to start…"), false));
            }
            if self.buffer.running_pid().is_none() {
                return Some((format!("{package} isn't running"), false));
            }
        }
        None
    }

    fn render_toolbar(&self, appearance: &Appearance, ctx: &AppContext) -> Box<dyn Element> {
        let theme = appearance.theme();
        let device_label = self
            .device_label(ctx)
            .unwrap_or_else(|| "No device".to_owned());
        let device_chip = chip_button(
            self.mouse_states.device_chip.clone(),
            device_label,
            Some(Icon::Phone),
            self.open_menu == Some(LogcatMenu::Devices),
            appearance,
        )
        .build()
        .on_click(|ctx, _, _| {
            ctx.dispatch_typed_action(LogcatAction::ToggleMenu(LogcatMenu::Devices))
        })
        .finish();
        let level_chip = chip_button(
            self.mouse_states.level_chip.clone(),
            self.level.name().to_owned(),
            None,
            self.open_menu == Some(LogcatMenu::Levels),
            appearance,
        )
        .build()
        .on_click(|ctx, _, _| {
            ctx.dispatch_typed_action(LogcatAction::ToggleMenu(LogcatMenu::Levels))
        })
        .finish();

        let regex_button = self.tool_button(
            Icon::Regex,
            "Regular expression",
            self.regex,
            self.mouse_states.regex.clone(),
            LogcatAction::ToggleRegex,
            appearance,
        );
        let (pause_icon, pause_tooltip) = if self.paused {
            (Icon::Play, "Resume")
        } else {
            (Icon::Stop, "Pause")
        };
        let pause_button = self.tool_button(
            pause_icon,
            pause_tooltip,
            self.paused,
            self.mouse_states.pause.clone(),
            LogcatAction::TogglePause,
            appearance,
        );
        let clear_button = self.tool_button(
            Icon::Trash,
            "Clear",
            false,
            self.mouse_states.clear.clone(),
            LogcatAction::Clear,
            appearance,
        );
        let end_button = self.tool_button(
            Icon::ArrowDown,
            "Scroll to the end and follow",
            self.follow,
            self.mouse_states.scroll_to_end.clone(),
            LogcatAction::ScrollToEnd,
            appearance,
        );

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(device_chip)
            .with_child(Container::new(level_chip).with_margin_left(4.).finish())
            .with_child(
                Shrinkable::new(
                    1.,
                    self.render_filter_input(
                        &self.package_editor,
                        FilterField::Package,
                        appearance,
                    ),
                )
                .finish(),
            )
            .with_child(
                Shrinkable::new(
                    1.,
                    self.render_filter_input(&self.tag_editor, FilterField::Tag, appearance),
                )
                .finish(),
            )
            .with_child(
                Shrinkable::new(
                    1.5,
                    self.render_filter_input(&self.text_editor, FilterField::Text, appearance),
                )
                .finish(),
            );
        for button in [regex_button, pause_button, clear_button, end_button] {
            row.add_child(Container::new(button).with_margin_left(2.).finish());
        }
        Container::new(row.finish())
            .with_horizontal_padding(8.)
            .with_vertical_padding(4.)
            .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
            .finish()
    }

    fn tool_button(
        &self,
        icon: Icon,
        tooltip: &'static str,
        active: bool,
        mouse_state: MouseStateHandle,
        action: LogcatAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let ui_builder = appearance.ui_builder().clone();
        icon_button(appearance, icon, active, mouse_state)
            .with_tooltip(move || ui_builder.tool_tip(tooltip.to_owned()).build().finish())
            .build()
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
            .finish()
    }

    fn render_filter_input(
        &self,
        editor: &ViewHandle<EditorView>,
        field: FilterField,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let input = Container::new(
            ConstrainedBox::new(Clipped::new(ChildView::new(editor).finish()).finish())
                .with_height(CONTROL_HEIGHT - 8.)
                .finish(),
        )
        .with_padding(Padding::uniform(4.))
        .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .with_margin_left(6.)
        .finish();
        EventHandler::new(input)
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action(LogcatAction::FocusEditor(field));
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    fn render_status(
        &self,
        message: &str,
        is_error: bool,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let color = if is_error {
            theme.ui_error_color()
        } else {
            theme.sub_text_color(theme.background()).into_solid()
        };
        Container::new(
            Text::new_inline(
                message.to_owned(),
                appearance.ui_font_family(),
                TOOLBAR_FONT_SIZE,
            )
            .with_color(color)
            .finish(),
        )
        .with_horizontal_padding(10.)
        .with_vertical_padding(4.)
        .finish()
    }

    fn render_list(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let row_count = self.buffer.row_count();
        if self.follow && row_count > 0 {
            self.list_state.scroll_to(row_count - 1);
        }
        let handle = self.handle.clone();
        let build_items = move |range: Range<usize>, app: &AppContext| {
            {
                match handle.upgrade(app) {
                    Some(view) => view.as_ref(app).render_rows(range, app),
                    None => Vec::new(),
                }
            }
            .into_iter()
        };
        let list = UniformList::new(self.list_state.clone(), row_count, build_items)
            .notify_visible_items(self.visible_tx.clone());
        let scrollable = Scrollable::vertical(
            self.scroll_state.clone(),
            list.finish_scrollable(),
            ScrollbarWidth::Auto,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            Fill::None,
        )
        .with_overlayed_scrollbar()
        .finish();
        EventHandler::new(scrollable)
            .with_always_handle()
            .on_scroll_wheel(|ctx, _, delta, _| {
                ctx.dispatch_typed_action(LogcatAction::ScrolledBy(delta.y()));
                DispatchEventResult::PropagateToParent
            })
            .on_left_mouse_down(|ctx, _, _| {
                ctx.dispatch_typed_action(LogcatAction::StopFollowing);
                DispatchEventResult::PropagateToParent
            })
            .finish()
    }

    fn render_rows(&self, range: Range<usize>, ctx: &AppContext) -> Vec<Box<dyn Element>> {
        let appearance = Appearance::as_ref(ctx);
        let style = RowStyle::new(appearance);
        self.buffer
            .rows(range)
            .map(|row| self.render_row(row, &style, appearance))
            .collect()
    }

    fn render_row(
        &self,
        row: Row<'_>,
        style: &RowStyle,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let entry = row.entry;
        let level_color = level_color(entry.level, appearance);
        let text = |content: String, color| {
            Text::new_inline(content, style.font_family, style.font_size)
                .with_color(color)
                .finish()
        };
        let column = |content: Option<Box<dyn Element>>, chars: f32| {
            ConstrainedBox::new(
                Clipped::new(content.unwrap_or_else(|| Empty::new().finish())).finish(),
            )
            .with_width(chars * style.char_width)
            .finish()
        };
        let mut flex = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        if row.is_first_line {
            flex.add_child(column(
                Some(text(entry.time.format_local(), style.meta_color)),
                TIME_CHARS,
            ));
            flex.add_child(column(
                Some(text(
                    format!("{}-{}", entry.pid, entry.tid),
                    style.meta_color,
                )),
                PID_CHARS,
            ));
            flex.add_child(column(
                Some(
                    Text::new_inline(
                        entry.level.letter().to_string(),
                        style.font_family,
                        style.font_size,
                    )
                    .with_color(level_color)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .finish(),
                ),
                2.,
            ));
            flex.add_child(column(
                Some(text(entry.tag.to_string(), style.tag_color)),
                TAG_CHARS,
            ));
        } else {
            flex.add_child(column(None, TIME_CHARS + PID_CHARS + 2. + TAG_CHARS));
        }
        let message_color = match entry.level {
            LogLevel::Error | LogLevel::Fatal | LogLevel::Warn => level_color,
            LogLevel::Verbose | LogLevel::Debug | LogLevel::Info => style.message_color,
        };
        let message: Box<dyn Element> = match parse_frame(row.line) {
            Some(frame) => self.render_frame_line(row.line, &frame, message_color, style),
            None => text(expand_tabs(row.line), message_color),
        };
        flex.add_child(Shrinkable::new(1., Clipped::new(message).finish()).finish());

        let mut container = Container::new(
            ConstrainedBox::new(flex.finish())
                .with_height(style.row_height)
                .finish(),
        )
        .with_padding_left(8.);
        if self.selected == Some(row.seq) {
            container = container.with_background(theme.surface_2());
        }
        let seq = row.seq;
        EventHandler::new(container.finish())
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action(LogcatAction::SelectRow(seq));
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    /// A stack-frame line with the `Class.method(File:line)` part as a link.
    fn render_frame_line(
        &self,
        line: &str,
        frame: &StackFrame,
        color: warpui::color::ColorU,
        style: &RowStyle,
    ) -> Box<dyn Element> {
        let piece = |content: &str, color| {
            Text::new_inline(expand_tabs(content), style.font_family, style.font_size)
                .with_color(color)
                .finish()
        };
        let line_owned = line.to_owned();
        let link = EventHandler::new(piece(&line[frame.span.clone()], style.link_color))
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action(LogcatAction::OpenFrame(line_owned.clone()));
                DispatchEventResult::StopPropagation
            })
            .finish();
        Flex::row()
            .with_child(piece(&line[..frame.span.start], color))
            .with_child(link)
            .with_child(piece(&line[frame.span.end..], color))
            .finish()
    }
}

/// Colors and metrics shared by every row of a frame.
struct RowStyle {
    font_family: warpui::fonts::FamilyId,
    font_size: f32,
    char_width: f32,
    row_height: f32,
    meta_color: warpui::color::ColorU,
    tag_color: warpui::color::ColorU,
    message_color: warpui::color::ColorU,
    link_color: warpui::color::ColorU,
}

impl RowStyle {
    fn new(appearance: &Appearance) -> Self {
        let theme = appearance.theme();
        let font_size = appearance.monospace_font_size();
        Self {
            font_family: appearance.monospace_font_family(),
            font_size,
            char_width: font_size * MONO_CHAR_WIDTH,
            row_height: (font_size * appearance.line_height_ratio()).ceil(),
            meta_color: theme.hint_text_color(theme.background()).into_solid(),
            tag_color: theme.sub_text_color(theme.background()).into_solid(),
            message_color: theme.main_text_color(theme.background()).into_solid(),
            link_color: theme.accent().into_solid(),
        }
    }
}

fn level_color(level: LogLevel, appearance: &Appearance) -> warpui::color::ColorU {
    let theme = appearance.theme();
    match level {
        LogLevel::Verbose => theme.hint_text_color(theme.background()).into_solid(),
        LogLevel::Debug => theme.ansi_fg_blue(),
        LogLevel::Info => theme.ansi_fg_green(),
        LogLevel::Warn => theme.ansi_fg_yellow(),
        LogLevel::Error => theme.ansi_fg_red(),
        LogLevel::Fatal => theme.ansi_fg_magenta(),
    }
}

/// Tabs (stack-trace indentation) as spaces, since text elements don't expand them.
fn expand_tabs(line: &str) -> String {
    line.replace('\t', "    ")
}

/// An entry as `logcat -v threadtime` prints it, one line per message line.
fn format_entry(entry: &LogEntry) -> String {
    let prefix = format!(
        "{} {:>5} {:>5} {} {}: ",
        entry.time.format_local(),
        entry.pid,
        entry.tid,
        entry.level.letter(),
        entry.tag
    );
    entry
        .message
        .split('\n')
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Files of the classes a frame names, most specific class first.
fn class_files_for_frame(frame: &StackFrame, index: &code_index::CodeIndex) -> Vec<PathBuf> {
    for name in frame.lookup_names() {
        let simple = name.rsplit('.').next().unwrap_or(&name);
        let files: Vec<PathBuf> = index
            .search(simple, SearchScope::Types, 50)
            .into_iter()
            .filter(|result| result.symbol.qualified_name() == name)
            .map(|result| index.abs_path(&result.symbol.rel_path))
            .collect();
        if !files.is_empty() {
            return files;
        }
    }
    Vec::new()
}

/// A labelled dropdown button styled like the toolbar's other controls.
fn chip_button(
    mouse_state: MouseStateHandle,
    label: String,
    icon: Option<Icon>,
    active: bool,
    appearance: &Appearance,
) -> Button {
    let theme = appearance.theme();
    let text_color = theme.main_text_color(theme.background());
    let sub_color = theme.sub_text_color(theme.background());
    let mut content = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
    if let Some(icon) = icon {
        content.add_child(
            Container::new(
                ConstrainedBox::new(icon.to_warpui_icon(sub_color).finish())
                    .with_width(14.)
                    .with_height(14.)
                    .finish(),
            )
            .with_margin_right(4.)
            .finish(),
        );
    }
    content.add_child(
        ConstrainedBox::new(
            Text::new_inline(label, appearance.ui_font_family(), TOOLBAR_FONT_SIZE)
                .with_color(text_color.into_solid())
                .with_clip(ClipConfig::end())
                .finish(),
        )
        .with_max_width(180.)
        .finish(),
    );
    content.add_child(
        Container::new(
            ConstrainedBox::new(Icon::ChevronDown.to_warpui_icon(sub_color).finish())
                .with_width(12.)
                .with_height(12.)
                .finish(),
        )
        .with_margin_left(4.)
        .finish(),
    );
    let default_styles = UiComponentStyles {
        font_color: Some(sub_color.into()),
        font_size: Some(TOOLBAR_FONT_SIZE),
        font_family_id: Some(appearance.ui_font_family()),
        height: Some(CONTROL_HEIGHT),
        border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.))),
        border_width: Some(0.),
        padding: Some(Coords {
            top: 0.,
            bottom: 0.,
            left: 6.,
            right: 6.,
        }),
        ..Default::default()
    };
    let hover_styles = UiComponentStyles {
        background: Some(theme.surface_2().into()),
        ..default_styles
    };
    let mut button = Button::new(
        mouse_state,
        default_styles,
        Some(hover_styles),
        Some(hover_styles),
        None,
    )
    .with_custom_label(content.finish());
    if active {
        button = button.active().with_active_styles(hover_styles);
    }
    button
}

impl Entity for LogcatView {
    type Event = LogcatViewEvent;
}

impl View for LogcatView {
    fn ui_name() -> &'static str {
        "LogcatView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let mut column = Flex::column().with_child(self.render_toolbar(appearance, app));
        if let Some((message, is_error)) = self.status_message(app) {
            column.add_child(self.render_status(&message, is_error, appearance));
        }
        column.add_child(Expanded::new(1., self.render_list(appearance)).finish());
        let content = Container::new(column.finish())
            .with_background(theme.background())
            .finish();

        let mut stack = Stack::new();
        stack.add_child(content);
        if self.open_menu.is_some() {
            stack.add_positioned_overlay_child(
                ChildView::new(&self.menu).finish(),
                OffsetPositioning::offset_from_parent(
                    warpui::geometry::vector::vec2f(8., 34.),
                    ParentOffsetBounds::ParentByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            );
        }
        stack.finish()
    }
}

impl TypedActionView for LogcatView {
    type Action = LogcatAction;

    fn handle_action(&mut self, action: &LogcatAction, ctx: &mut ViewContext<Self>) {
        match action {
            LogcatAction::ToggleMenu(menu) => self.toggle_menu(*menu, ctx),
            LogcatAction::SelectDevice(serial) => {
                self.open_menu = None;
                let serial = serial.clone();
                AndroidModel::handle(ctx).update(ctx, |android, ctx| android.select(serial, ctx));
                ctx.notify();
            }
            LogcatAction::LaunchAvd(avd) => {
                self.open_menu = None;
                let avd = avd.clone();
                AndroidModel::handle(ctx).update(ctx, |android, ctx| android.launch_avd(avd, ctx));
                ctx.notify();
            }
            LogcatAction::SetLevel(level) => {
                self.open_menu = None;
                self.level = *level;
                self.apply_filters(ctx);
            }
            LogcatAction::ToggleRegex => {
                self.regex = !self.regex;
                self.apply_filters(ctx);
            }
            LogcatAction::TogglePause => {
                self.paused = !self.paused;
                if !self.paused {
                    let backlog = std::mem::take(&mut self.paused_backlog);
                    self.push_entries(backlog);
                }
                ctx.notify();
            }
            LogcatAction::Clear => {
                self.buffer.clear();
                self.paused_backlog.clear();
                self.selected = None;
                self.follow = true;
                ctx.notify();
            }
            LogcatAction::StopFollowing => {
                if self.follow {
                    self.follow = false;
                    ctx.notify();
                }
            }
            LogcatAction::ScrollToEnd => {
                self.follow = true;
                ctx.notify();
            }
            LogcatAction::ScrolledBy(delta) => {
                if *delta > 0. {
                    self.follow = false;
                } else if *delta < 0. && self.visible.end >= self.buffer.row_count() {
                    self.follow = true;
                }
                ctx.notify();
            }
            LogcatAction::SelectRow(seq) => {
                self.selected = Some(*seq);
                ctx.focus_self();
                ctx.notify();
            }
            LogcatAction::CopySelection => {
                if let Some(text) = self.selection_text() {
                    ctx.clipboard().write(ClipboardContent::plain_text(text));
                }
            }
            LogcatAction::OpenFrame(line) => self.open_frame(line, ctx),
            LogcatAction::FocusEditor(field) => {
                let editor = match field {
                    FilterField::Package => &self.package_editor,
                    FilterField::Tag => &self.tag_editor,
                    FilterField::Text => &self.text_editor,
                };
                ctx.focus(editor);
            }
        }
    }
}

impl BackingView for LogcatView {
    type PaneHeaderOverflowMenuAction = LogcatOverflowAction;
    type CustomAction = ();
    type AssociatedData = ();

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        action: &Self::PaneHeaderOverflowMenuAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            LogcatOverflowAction::CopyVisible => {
                ctx.clipboard()
                    .write(ClipboardContent::plain_text(self.visible_text()));
            }
            LogcatOverflowAction::Clear => self.handle_action(&LogcatAction::Clear, ctx),
        }
    }

    fn pane_header_overflow_menu_items(
        &self,
        _ctx: &AppContext,
    ) -> Vec<MenuItem<Self::PaneHeaderOverflowMenuAction>> {
        vec![
            MenuItem::Item(
                MenuItemFields::new("Copy shown log")
                    .with_on_select_action(LogcatOverflowAction::CopyVisible),
            ),
            MenuItem::Item(
                MenuItemFields::new("Clear").with_on_select_action(LogcatOverflowAction::Clear),
            ),
        ]
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(LogcatViewEvent::Pane(PaneEvent::Close));
    }

    fn focus_contents(&mut self, ctx: &mut ViewContext<Self>) {
        self.focus(ctx);
    }

    fn render_header_content(
        &self,
        _ctx: &view::HeaderRenderContext<'_>,
        app: &AppContext,
    ) -> HeaderContent {
        HeaderContent::Standard(StandardHeader {
            title: HEADER_TEXT.to_owned(),
            title_secondary: self.device_label(app),
            title_style: None,
            title_clip_config: ClipConfig::start(),
            title_max_width: None,
            left_of_title: None,
            right_of_title: None,
            left_of_overflow: None,
            options: StandardHeaderOptions::default(),
        })
    }

    fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, _ctx: &mut ViewContext<Self>) {
        self.focus_handle = Some(focus_handle);
    }
}

/// The package of the project's first Android app module, for the package filter.
pub(crate) fn default_package(project_root: &Path) -> Option<String> {
    warpium_android::project::find_app_modules(project_root)
        .first()
        .and_then(|module| module.package_for_variant("debug"))
}
