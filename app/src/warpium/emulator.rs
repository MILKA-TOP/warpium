//! Warpium's Devices pane: the screens of running Android emulators, one sub-tab each, streamed
//! over the emulator's gRPC endpoint (`warpium_emulator`).
//!
//! Each tab owns a [`Session`] (a connection on its own thread) and a [`Viewer`] that decides when
//! frames are needed. Frames go through a one-frame mailbox, at most 60 per second, and each one
//! becomes a new [`StaticImage`]: the renderer caches textures by the image's `Arc`, so a new
//! frame uploads a new texture and the previous one is freed at the end of the next frame. Frames
//! are only streamed while the tab is painted; the frame element keeps a visible tab repainting
//! often enough to tell "visible" from "hidden", so an unseen tab costs nothing.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use instant::Instant;
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use warp_core::features::FeatureFlag;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpium_emulator::geometry::{Rect, fit_frame};
use warpium_emulator::viewer::PAINT_HEARTBEAT;
use warpium_emulator::{
    RunningEmulator, Session, SessionError, SessionEvent, SessionOptions, StreamRequest, Viewer,
    ViewerStatus,
};
use warpui::r#async::Timer;
use warpui::elements::{
    Align, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Empty, Expanded, Flex,
    MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, Point, Shrinkable, Stack,
    Text,
};
use warpui::event::DispatchedEvent;
use warpui::image_cache::StaticImage;
use warpui::keymap::EditableBinding;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AfterLayoutContext, AppContext, Element, Entity, EventContext, LayoutContext, ModelHandle,
    PaintContext, SingletonEntity, SizeConstraint, TypedActionView, View, ViewContext, WindowId,
};

use super::android::{AndroidEvent, AndroidModel};
use super::sync_settings::ANDROID_CONTEXT_FLAG;
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::pane::view::{self, HeaderContent, StandardHeader, StandardHeaderOptions};
use crate::pane_group::{BackingView, PaneConfiguration, PaneEvent};
use crate::ui_components::buttons::icon_button;
use crate::ui_components::tab_selector::{SettingsTab, render_tab_selector};
use crate::util::bindings::BindingGroup;
use crate::workspace::{PaneViewLocator, WorkspaceAction};

const HEADER_TEXT: &str = "Devices";
/// Frames are shown at most this often (60 per second); newer ones replace waiting ones.
const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// How often a streaming tab checks that it is still painted.
const WATCHDOG_INTERVAL: Duration = Duration::from_millis(250);
/// How often frame statistics are logged (at debug level) while frames arrive.
const STATS_INTERVAL: Duration = Duration::from_secs(5);
const STATUS_FONT_SIZE: f32 = 13.;
const BUTTON_FONT_SIZE: f32 = 12.;
const FRAME_MARGIN: f32 = 8.;

/// Whether the Devices pane is available: the feature flag and the Android tools are on.
pub(crate) fn is_enabled(ctx: &AppContext) -> bool {
    FeatureFlag::WarpiumEmbeddedEmulator.is_enabled() && super::android::is_enabled(ctx)
}

pub(crate) fn init_bindings(ctx: &mut AppContext) {
    use warpui::keymap::macros::*;

    ctx.register_editable_bindings([EditableBinding::new(
        "workspace:show_running_emulator",
        "Show running emulator in Warpium",
        WorkspaceAction::ShowRunningEmulator,
    )
    .with_group(BindingGroup::Navigation.as_str())
    .with_context_predicate(id!("Workspace") & id!(ANDROID_CONTEXT_FLAG))
    .with_enabled(|| FeatureFlag::WarpiumEmbeddedEmulator.is_enabled())]);

    #[cfg(debug_assertions)]
    debug_fakes::init_bindings(ctx);
}

/// Tracks the Devices pane of each window, so showing an emulator reuses the existing one.
#[derive(Default)]
pub(crate) struct DevicesPaneManager {
    panes: HashMap<WindowId, PaneViewLocator>,
    #[cfg(debug_assertions)]
    fakes: debug_fakes::FakeEmulators,
}

impl DevicesPaneManager {
    pub(crate) fn find_pane(&self, window_id: WindowId) -> Option<PaneViewLocator> {
        self.panes.get(&window_id).copied()
    }

    pub(crate) fn register_pane(&mut self, window_id: WindowId, locator: PaneViewLocator) {
        self.panes.insert(window_id, locator);
    }

    pub(crate) fn deregister_pane(&mut self, window_id: &WindowId) {
        self.panes.remove(window_id);
    }

    /// Emulators to list next to the discovered ones (debug builds' in-process fakes).
    fn extra_emulators(&self) -> Vec<RunningEmulator> {
        #[cfg(debug_assertions)]
        {
            self.fakes.entries()
        }
        #[cfg(not(debug_assertions))]
        {
            Vec::new()
        }
    }
}

impl Entity for DevicesPaneManager {
    type Event = ();
}

impl SingletonEntity for DevicesPaneManager {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DevicesViewEvent {
    Pane(PaneEvent),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DevicesAction {
    SelectTab(u64),
    CloseTab(u64),
    Retry(u64),
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DevicesOverflowAction {
    Refresh,
}

/// Shared between a tab and the frame element of its latest render: the element records each
/// paint here and wakes the view when the view asked for it or the frame area changed size.
struct PaintProbe {
    tab_id: u64,
    last_paint: Cell<Option<Instant>>,
    /// The frame area in physical pixels at the last paint.
    size: Cell<(u32, u32)>,
    /// Set while the view waits for a paint to start streaming.
    wake: Cell<bool>,
    /// Set while streaming, so the element keeps the window repainting.
    heartbeat: Cell<bool>,
    painted_generation: Cell<u64>,
    frames_painted: Cell<u64>,
    paint_tx: async_channel::Sender<u64>,
}

impl PaintProbe {
    fn record_paint(&self, size: (u32, u32)) {
        self.last_paint.set(Some(Instant::now()));
        let resized = self.size.replace(size) != size;
        if resized || self.wake.replace(false) {
            let _ = self.paint_tx.try_send(self.tab_id);
        }
    }
}

#[derive(Default)]
struct FrameStats {
    shown: u64,
    copy_time: Duration,
    logged_at: Option<Instant>,
    logged_received: u64,
}

struct DeviceTab {
    id: u64,
    emulator: RunningEmulator,
    label: String,
    session: Option<Session>,
    viewer: Viewer,
    image: Option<Arc<StaticImage>>,
    /// Bumped for every frame shown, so the element can count painted frames.
    image_generation: u64,
    probe: Rc<PaintProbe>,
    last_shown: Option<Instant>,
    show_scheduled: bool,
    watchdog_running: bool,
    stats: FrameStats,
    tab_mouse_state: MouseStateHandle,
}

impl DeviceTab {
    fn is_same_emulator(&self, emulator: &RunningEmulator) -> bool {
        self.emulator.pid == emulator.pid
            && self.emulator.console_port == emulator.console_port
            && self.emulator.grpc_port == emulator.grpc_port
    }

    fn serial(&self) -> String {
        self.emulator.adb_serial()
    }

    /// Lets the next paint wake the view when it would start streaming.
    fn sync_probe(&self) {
        self.probe
            .wake
            .set(self.viewer.is_connected() && !self.viewer.is_streaming());
        self.probe.heartbeat.set(self.viewer.is_streaming());
    }
}

#[derive(Default)]
struct MouseStates {
    refresh: MouseStateHandle,
    close_tab: MouseStateHandle,
    retry: MouseStateHandle,
    empty_refresh: MouseStateHandle,
}

pub(crate) struct DevicesView {
    tabs: Vec<DeviceTab>,
    active: Option<u64>,
    next_tab_id: u64,
    /// The serial to select once discovery lists it.
    pending_focus: Option<String>,
    refreshing: bool,
    refresh_generation: u64,
    session_tx: async_channel::Sender<(u64, SessionEvent)>,
    paint_tx: async_channel::Sender<u64>,
    pane_configuration: ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,
    mouse_states: MouseStates,
}

impl DevicesView {
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        let pane_configuration = ctx.add_model(|_| PaneConfiguration::new(HEADER_TEXT));

        let (session_tx, session_rx) = async_channel::unbounded();
        ctx.spawn_stream_local(
            session_rx,
            |me, (tab_id, event), ctx| me.on_session_event(tab_id, event, ctx),
            |_, _| {},
        );
        let (paint_tx, paint_rx) = async_channel::unbounded();
        ctx.spawn_stream_local(
            paint_rx,
            |me, tab_id, ctx| me.on_paint(tab_id, ctx),
            |_, _| {},
        );

        ctx.subscribe_to_model(
            &AndroidModel::handle(ctx),
            |me, _, event, ctx| match event {
                // A new or vanished `emulator-*` device is the cheapest signal that the set of
                // running emulators changed.
                AndroidEvent::DevicesChanged | AndroidEvent::AvdBooted => me.refresh(ctx),
                AndroidEvent::SelectionChanged
                | AndroidEvent::AvdsChanged
                | AndroidEvent::AvdLaunchFailed
                | AndroidEvent::AppLaunched(_) => {}
            },
        );

        let mut view = Self {
            tabs: Vec::new(),
            active: None,
            next_tab_id: 0,
            pending_focus: None,
            refreshing: false,
            refresh_generation: 0,
            session_tx,
            paint_tx,
            pane_configuration,
            focus_handle: None,
            mouse_states: MouseStates::default(),
        };
        view.refresh(ctx);
        view
    }

    pub(crate) fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    pub(crate) fn focus(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
    }

    /// Re-reads the running emulators and selects the one with this adb serial when given.
    pub(crate) fn show(&mut self, serial: Option<String>, ctx: &mut ViewContext<Self>) {
        if let Some(serial) = serial {
            match self.tabs.iter().find(|tab| tab.serial() == serial) {
                Some(tab) => self.active = Some(tab.id),
                None => self.pending_focus = Some(serial),
            }
        }
        self.refresh(ctx);
        ctx.notify();
    }

    /// Ends every session; called when the pane closes for good.
    pub(crate) fn stop_all(&mut self, ctx: &mut ViewContext<Self>) {
        self.tabs.clear();
        self.active = None;
        self.refresh_generation += 1;
        ctx.notify();
    }

    fn refresh(&mut self, ctx: &mut ViewContext<Self>) {
        self.refresh_generation += 1;
        let generation = self.refresh_generation;
        self.refreshing = true;
        let extra = DevicesPaneManager::as_ref(ctx).extra_emulators();
        ctx.spawn(
            async move {
                let mut emulators = warpium_emulator::discovery::running_emulators();
                emulators.extend(extra);
                emulators
            },
            move |me, emulators, ctx| {
                if me.refresh_generation == generation {
                    me.apply_discovery(emulators, ctx);
                }
            },
        );
        ctx.notify();
    }

    fn apply_discovery(&mut self, emulators: Vec<RunningEmulator>, ctx: &mut ViewContext<Self>) {
        self.refreshing = false;
        let now = Instant::now();
        for tab in &mut self.tabs {
            let running = emulators
                .iter()
                .any(|emulator| tab.is_same_emulator(emulator));
            if !running && tab.session.is_some() {
                log::info!("[Devices] {} is no longer running", tab.serial());
                tab.session = None;
                tab.image = None;
                tab.viewer
                    .on_session_event(&SessionEvent::Failed(SessionError::Exited), now);
                tab.sync_probe();
            }
        }
        for emulator in emulators {
            if !self.tabs.iter().any(|tab| tab.is_same_emulator(&emulator)) {
                self.add_tab(emulator);
            }
        }
        self.relabel_tabs();
        if let Some(serial) = self.pending_focus.take() {
            match self.tabs.iter().find(|tab| tab.serial() == serial) {
                Some(tab) => self.active = Some(tab.id),
                None => log::info!("[Devices] {serial} has no gRPC discovery entry"),
            }
        }
        if self.active_tab().is_none() {
            self.active = self.tabs.first().map(|tab| tab.id);
        }
        ctx.notify();
    }

    fn add_tab(&mut self, emulator: RunningEmulator) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let probe = Rc::new(PaintProbe {
            tab_id: id,
            last_paint: Cell::new(None),
            size: Cell::new((0, 0)),
            wake: Cell::new(false),
            heartbeat: Cell::new(false),
            painted_generation: Cell::new(0),
            frames_painted: Cell::new(0),
            paint_tx: self.paint_tx.clone(),
        });
        let mut tab = DeviceTab {
            id,
            label: tab_label(&emulator),
            emulator,
            session: None,
            viewer: Viewer::new(),
            image: None,
            image_generation: 0,
            probe,
            last_shown: None,
            show_scheduled: false,
            watchdog_running: false,
            stats: FrameStats::default(),
            tab_mouse_state: MouseStateHandle::default(),
        };
        self.start_session(&mut tab);
        self.tabs.push(tab);
    }

    fn start_session(&self, tab: &mut DeviceTab) {
        tab.viewer = Viewer::new();
        tab.image = None;
        tab.stats = FrameStats::default();
        let tab_id = tab.id;
        let session_tx = self.session_tx.clone();
        let started = Session::start(
            tab.emulator.clone(),
            SessionOptions::default(),
            move |event| {
                let _ = session_tx.try_send((tab_id, event));
            },
        );
        match started {
            Ok(session) => tab.session = Some(session),
            Err(err) => {
                log::error!("[Devices] Couldn't start the emulator session thread: {err}");
                tab.session = None;
                tab.viewer.on_session_event(
                    &SessionEvent::Failed(SessionError::Other(
                        "couldn't start a background thread".to_owned(),
                    )),
                    Instant::now(),
                );
            }
        }
        tab.sync_probe();
    }

    /// Tab labels are AVD names, with the serial added where two tabs share one.
    fn relabel_tabs(&mut self) {
        let names: Vec<String> = self
            .tabs
            .iter()
            .map(|tab| tab_label(&tab.emulator))
            .collect();
        for (tab, name) in self.tabs.iter_mut().zip(&names) {
            let shared = names.iter().filter(|other| *other == name).count() > 1;
            tab.label = if shared {
                format!("{name} ({})", tab.serial())
            } else {
                name.clone()
            };
        }
    }

    fn tab_mut(&mut self, tab_id: u64) -> Option<&mut DeviceTab> {
        self.tabs.iter_mut().find(|tab| tab.id == tab_id)
    }

    fn active_tab(&self) -> Option<&DeviceTab> {
        let active = self.active?;
        self.tabs.iter().find(|tab| tab.id == active)
    }

    fn on_session_event(&mut self, tab_id: u64, event: SessionEvent, ctx: &mut ViewContext<Self>) {
        if event == SessionEvent::FrameReady {
            self.schedule_show_frame(tab_id, ctx);
            return;
        }
        let is_active = self.active == Some(tab_id);
        let Some(tab) = self.tab_mut(tab_id) else {
            return;
        };
        match &event {
            SessionEvent::FrameReady | SessionEvent::Connected => {}
            SessionEvent::Failed(err) => {
                log::info!("[Devices] {} stopped: {err}", tab.serial());
                tab.session = None;
                tab.image = None;
            }
        }
        let request = tab.viewer.on_session_event(&event, Instant::now());
        self.apply_request(tab_id, request, ctx);
        // A status change re-renders the tab, and its paint starts the stream once connected.
        if is_active {
            ctx.notify();
        }
    }

    fn on_paint(&mut self, tab_id: u64, ctx: &mut ViewContext<Self>) {
        let Some(tab) = self.tab_mut(tab_id) else {
            return;
        };
        let Some(painted_at) = tab.probe.last_paint.get() else {
            return;
        };
        let (width, height) = tab.probe.size.get();
        let request = tab.viewer.on_paint(painted_at, width, height);
        self.apply_request(tab_id, request, ctx);
    }

    fn apply_request(
        &mut self,
        tab_id: u64,
        request: Option<StreamRequest>,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(tab) = self.tab_mut(tab_id) else {
            return;
        };
        tab.sync_probe();
        let (Some(request), Some(session)) = (request, &tab.session) else {
            return;
        };
        match request {
            StreamRequest::Stream { width, height } => {
                log::debug!(
                    "[Devices] {} requesting frames at {width}x{height}",
                    tab.serial()
                );
                session.stream(width, height);
                // Repaint now so the element starts its heartbeat right away.
                ctx.notify();
                self.start_watchdog(tab_id, ctx);
            }
            StreamRequest::Pause => {
                log::debug!(
                    "[Devices] {} not painted for a while; pausing frames",
                    tab.serial()
                );
                session.pause();
            }
        }
    }

    fn start_watchdog(&mut self, tab_id: u64, ctx: &mut ViewContext<Self>) {
        let Some(tab) = self.tab_mut(tab_id) else {
            return;
        };
        if tab.watchdog_running {
            return;
        }
        tab.watchdog_running = true;
        ctx.spawn(
            async move { Timer::after(WATCHDOG_INTERVAL).await },
            move |me, _, ctx| me.on_watchdog(tab_id, ctx),
        );
    }

    fn on_watchdog(&mut self, tab_id: u64, ctx: &mut ViewContext<Self>) {
        let Some(tab) = self.tab_mut(tab_id) else {
            return;
        };
        tab.watchdog_running = false;
        let now = Instant::now();
        let request = tab.viewer.on_tick(now, tab.probe.last_paint.get());
        log_stats(tab, now);
        let streaming = tab.viewer.is_streaming();
        self.apply_request(tab_id, request, ctx);
        if streaming {
            self.start_watchdog(tab_id, ctx);
        }
    }

    fn schedule_show_frame(&mut self, tab_id: u64, ctx: &mut ViewContext<Self>) {
        let Some(tab) = self.tab_mut(tab_id) else {
            return;
        };
        if tab.show_scheduled {
            return;
        }
        let since_last = tab.last_shown.map(|shown| Instant::now() - shown);
        match since_last {
            Some(elapsed) if elapsed < MIN_FRAME_INTERVAL => {
                tab.show_scheduled = true;
                let wait = MIN_FRAME_INTERVAL - elapsed;
                ctx.spawn(
                    async move { Timer::after(wait).await },
                    move |me, _, ctx| {
                        if let Some(tab) = me.tab_mut(tab_id) {
                            tab.show_scheduled = false;
                        }
                        me.show_frame(tab_id, ctx);
                    },
                );
            }
            Some(_) | None => self.show_frame(tab_id, ctx),
        }
    }

    fn show_frame(&mut self, tab_id: u64, ctx: &mut ViewContext<Self>) {
        let is_active = self.active == Some(tab_id);
        let Some(tab) = self.tab_mut(tab_id) else {
            return;
        };
        let Some(frame) = tab.session.as_ref().and_then(Session::take_frame) else {
            return;
        };
        tab.viewer.on_frame(&frame);
        let started = Instant::now();
        tab.last_shown = Some(started);
        if frame.is_display_off() {
            tab.image = None;
        } else {
            let (width, height) = (frame.width, frame.height);
            // Free when the gRPC buffer holds nothing else, a copy otherwise.
            match StaticImage::from_rgba(width, height, Vec::from(frame.pixels)) {
                Some(image) => {
                    tab.image = Some(Arc::new(image));
                    tab.image_generation += 1;
                    tab.stats.shown += 1;
                    tab.stats.copy_time += started.elapsed();
                }
                None => {
                    log::warn!("[Devices] Dropped a {width}x{height} frame with a bad size");
                }
            }
        }
        if is_active {
            ctx.notify();
        }
    }

    fn close_tab(&mut self, tab_id: u64, ctx: &mut ViewContext<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        self.tabs.remove(index);
        if self.active == Some(tab_id) {
            self.active = self
                .tabs
                .get(index.min(self.tabs.len().saturating_sub(1)))
                .map(|tab| tab.id);
        }
        self.relabel_tabs();
        ctx.notify();
    }

    fn retry(&mut self, tab_id: u64, ctx: &mut ViewContext<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let mut tab = self.tabs.remove(index);
        self.start_session(&mut tab);
        self.tabs.insert(index, tab);
        ctx.notify();
    }

    fn render_tab_strip(&self, appearance: &Appearance) -> Box<dyn Element> {
        let tabs = self
            .tabs
            .iter()
            .map(|tab| SettingsTab::new(tab.label.clone(), tab.tab_mouse_state.clone()))
            .collect();
        let selected = self
            .active_tab()
            .map(|tab| tab.label.clone())
            .unwrap_or_default();
        let ids: Vec<(String, u64)> = self
            .tabs
            .iter()
            .map(|tab| (tab.label.clone(), tab.id))
            .collect();
        let selector = render_tab_selector(
            tabs,
            &selected,
            move |label, ctx| {
                if let Some((_, id)) = ids.iter().find(|(tab_label, _)| tab_label == label) {
                    ctx.dispatch_typed_action(DevicesAction::SelectTab(*id));
                }
            },
            appearance,
        );
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(Shrinkable::new(1., selector).finish());
        if let Some(tab) = self.active_tab() {
            row.add_child(self.tool_button(
                Icon::X,
                "Close tab",
                self.mouse_states.close_tab.clone(),
                DevicesAction::CloseTab(tab.id),
                appearance,
            ));
        }
        row.add_child(self.tool_button(
            Icon::Refresh,
            "Find running emulators",
            self.mouse_states.refresh.clone(),
            DevicesAction::Refresh,
            appearance,
        ));
        Container::new(row.finish())
            .with_horizontal_padding(12.)
            .with_margin_top(4.)
            .finish()
    }

    fn tool_button(
        &self,
        icon: Icon,
        tooltip: &'static str,
        mouse_state: MouseStateHandle,
        action: DevicesAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let ui_builder = appearance.ui_builder().clone();
        Container::new(
            icon_button(appearance, icon, false, mouse_state)
                .with_tooltip(move || ui_builder.tool_tip(tooltip.to_owned()).build().finish())
                .build()
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
                .finish(),
        )
        .with_margin_left(2.)
        .finish()
    }

    fn text_button(
        &self,
        label: &str,
        mouse_state: &MouseStateHandle,
        action: DevicesAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        appearance
            .ui_builder()
            .button(ButtonVariant::Secondary, mouse_state.clone())
            .with_text_label(label.to_owned())
            .with_style(UiComponentStyles {
                font_size: Some(BUTTON_FONT_SIZE),
                padding: Some(Coords::uniform(6.).left(12.).right(12.)),
                ..Default::default()
            })
            .build()
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
            .finish()
    }

    /// A centered message, an error when `is_error`, with an optional button under it.
    fn render_message(
        &self,
        message: String,
        is_error: bool,
        button: Option<Box<dyn Element>>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let color = if is_error {
            theme.ui_error_color()
        } else {
            theme.sub_text_color(theme.background()).into_solid()
        };
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_child(
                ConstrainedBox::new(
                    Text::new(message, appearance.ui_font_family(), STATUS_FONT_SIZE)
                        .with_color(color)
                        .finish(),
                )
                .with_max_width(360.)
                .finish(),
            );
        if let Some(button) = button {
            column.add_child(Container::new(button).with_margin_top(12.).finish());
        }
        Align::new(
            Container::new(column.finish())
                .with_uniform_padding(16.)
                .finish(),
        )
        .finish()
    }

    fn render_empty(&self, appearance: &Appearance) -> Box<dyn Element> {
        let message = if self.refreshing {
            "Looking for running emulators…".to_owned()
        } else {
            "No running emulator found. Start one from the device menu, Android Studio or the \
             command line, then refresh."
                .to_owned()
        };
        let button = (!self.refreshing).then(|| {
            self.text_button(
                "Refresh",
                &self.mouse_states.empty_refresh,
                DevicesAction::Refresh,
                appearance,
            )
        });
        self.render_message(message, false, button, appearance)
    }

    fn render_tab(&self, tab: &DeviceTab, appearance: &Appearance) -> Box<dyn Element> {
        let screen = DeviceScreen {
            image: tab.image.clone(),
            generation: tab.image_generation,
            probe: tab.probe.clone(),
            size: None,
            origin: None,
        }
        .finish();
        let mut stack = Stack::new();
        stack.add_child(
            Container::new(screen)
                .with_uniform_padding(FRAME_MARGIN)
                .finish(),
        );
        if let Some((message, is_error)) = status_message(tab.viewer.status(), &tab.emulator) {
            let button = match tab.viewer.status() {
                ViewerStatus::Failed(SessionError::Exited) => Some(self.text_button(
                    "Close tab",
                    &self.mouse_states.retry,
                    DevicesAction::CloseTab(tab.id),
                    appearance,
                )),
                ViewerStatus::Failed(_) => Some(self.text_button(
                    "Retry",
                    &self.mouse_states.retry,
                    DevicesAction::Retry(tab.id),
                    appearance,
                )),
                ViewerStatus::Connecting
                | ViewerStatus::WaitingForFrame
                | ViewerStatus::Showing
                | ViewerStatus::ScreenOff => None,
            };
            stack.add_child(self.render_message(message, is_error, button, appearance));
        }
        stack.finish()
    }
}

/// The message over a tab's screen and whether it is an error; `None` while frames show.
fn status_message(status: &ViewerStatus, emulator: &RunningEmulator) -> Option<(String, bool)> {
    let message = match status {
        ViewerStatus::Showing => return None,
        ViewerStatus::Connecting => format!("Connecting to {}…", emulator.adb_serial()),
        ViewerStatus::WaitingForFrame => "Waiting for the screen…".to_owned(),
        ViewerStatus::ScreenOff => "The emulator's screen is off.".to_owned(),
        ViewerStatus::Failed(err) => return Some((failure_message(err, emulator), true)),
    };
    Some((message, false))
}

fn failure_message(err: &SessionError, emulator: &RunningEmulator) -> String {
    match err {
        SessionError::TlsRequired => "This emulator's gRPC endpoint requires TLS, which Warpium \
                                      doesn't support yet. Start it without the -grpc-tls \
                                      options."
            .to_owned(),
        SessionError::Unauthorized => "The emulator rejected Warpium's access token. Restart the \
                                       emulator, then retry."
            .to_owned(),
        SessionError::Unreachable => format!(
            "Couldn't connect to the emulator's gRPC port {}.",
            emulator.grpc_port
        ),
        SessionError::Exited => "The emulator exited.".to_owned(),
        SessionError::Other(reason) => format!("Showing the screen failed: {reason}"),
    }
}

/// The AVD name with underscores as spaces, or the adb serial when there is none.
fn tab_label(emulator: &RunningEmulator) -> String {
    emulator
        .avd_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .map(|name| name.replace('_', " "))
        .unwrap_or_else(|| emulator.adb_serial())
}

fn log_stats(tab: &mut DeviceTab, now: Instant) {
    let Some(session) = &tab.session else {
        return;
    };
    let received = session.frames_received();
    let due = tab
        .stats
        .logged_at
        .is_none_or(|logged| now - logged >= STATS_INTERVAL);
    if !due || received == tab.stats.logged_received {
        return;
    }
    let shown = tab.stats.shown;
    let copy_ms = if shown == 0 {
        0.
    } else {
        tab.stats.copy_time.as_secs_f64() * 1000. / shown as f64
    };
    log::debug!(
        "[Devices] {} frames received={received} dropped={} shown={shown} painted={} \
         avg_copy_ms={copy_ms:.2} size={:?}",
        tab.serial(),
        session.frames_dropped(),
        tab.probe.frames_painted.get(),
        tab.probe.size.get(),
    );
    tab.stats.logged_at = Some(now);
    tab.stats.logged_received = received;
}

/// Draws a tab's latest frame aspect-fit in its bounds and reports each paint to the tab's
/// [`PaintProbe`].
struct DeviceScreen {
    image: Option<Arc<StaticImage>>,
    generation: u64,
    probe: Rc<PaintProbe>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Element for DeviceScreen {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _ctx: &mut LayoutContext,
        _app: &AppContext,
    ) -> Vector2F {
        let finite = |max: f32, min: f32| if max.is_finite() { max } else { min };
        let size = vec2f(
            finite(constraint.max.x(), constraint.min.x()),
            finite(constraint.max.y(), constraint.min.y()),
        );
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, _ctx: &mut AfterLayoutContext, _app: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let Some(size) = self.size else {
            return;
        };
        let scale = ctx.scene.scale_factor();
        let pixels = (
            (size.x() * scale).round().max(0.) as u32,
            (size.y() * scale).round().max(0.) as u32,
        );
        self.probe.record_paint(pixels);
        if let Some(image) = &self.image {
            let container = Rect {
                x: origin.x(),
                y: origin.y(),
                width: size.x(),
                height: size.y(),
            };
            if let Some(rect) = fit_frame(container, image.width(), image.height()) {
                ctx.scene.draw_image(
                    RectF::new(vec2f(rect.x, rect.y), vec2f(rect.width, rect.height)),
                    image.clone(),
                    1.,
                    CornerRadius::default(),
                );
                if self.probe.painted_generation.replace(self.generation) != self.generation {
                    self.probe
                        .frames_painted
                        .set(self.probe.frames_painted.get() + 1);
                }
            }
        }
        if self.probe.heartbeat.get() {
            ctx.repaint_after(PAINT_HEARTBEAT);
        }
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        _event: &DispatchedEvent,
        _ctx: &mut EventContext,
        _app: &AppContext,
    ) -> bool {
        false
    }
}

impl Entity for DevicesView {
    type Event = DevicesViewEvent;
}

impl View for DevicesView {
    fn ui_name() -> &'static str {
        "WarpiumDevicesView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let content = match self.active_tab() {
            Some(tab) => Flex::column()
                .with_child(self.render_tab_strip(appearance))
                .with_child(Expanded::new(1., self.render_tab(tab, appearance)).finish())
                .finish(),
            None if self.tabs.is_empty() => self.render_empty(appearance),
            None => Empty::new().finish(),
        };
        Container::new(content)
            .with_background(theme.background())
            .finish()
    }
}

impl TypedActionView for DevicesView {
    type Action = DevicesAction;

    fn handle_action(&mut self, action: &DevicesAction, ctx: &mut ViewContext<Self>) {
        match action {
            DevicesAction::SelectTab(tab_id) => {
                self.active = Some(*tab_id);
                ctx.notify();
            }
            DevicesAction::CloseTab(tab_id) => self.close_tab(*tab_id, ctx),
            DevicesAction::Retry(tab_id) => self.retry(*tab_id, ctx),
            DevicesAction::Refresh => self.refresh(ctx),
        }
    }
}

impl BackingView for DevicesView {
    type PaneHeaderOverflowMenuAction = DevicesOverflowAction;
    type CustomAction = ();
    type AssociatedData = ();

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        action: &Self::PaneHeaderOverflowMenuAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            DevicesOverflowAction::Refresh => self.refresh(ctx),
        }
    }

    fn pane_header_overflow_menu_items(
        &self,
        _ctx: &AppContext,
    ) -> Vec<crate::menu::MenuItem<Self::PaneHeaderOverflowMenuAction>> {
        vec![crate::menu::MenuItem::Item(
            crate::menu::MenuItemFields::new("Find running emulators")
                .with_on_select_action(DevicesOverflowAction::Refresh),
        )]
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(DevicesViewEvent::Pane(PaneEvent::Close));
    }

    fn focus_contents(&mut self, ctx: &mut ViewContext<Self>) {
        self.focus(ctx);
    }

    fn render_header_content(
        &self,
        _ctx: &view::HeaderRenderContext<'_>,
        _app: &AppContext,
    ) -> HeaderContent {
        HeaderContent::Standard(StandardHeader {
            title: HEADER_TEXT.to_owned(),
            title_secondary: self.active_tab().map(|tab| tab.label.clone()),
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

/// Debug builds only: in-process fake emulators (`warpium_emulator::test_support`) that the pane
/// lists like real ones, so it can be tried where no emulator can run (such as a macOS VM).
#[cfg(debug_assertions)]
pub(crate) mod debug_fakes {
    use warpium_emulator::geometry::DisplaySize;
    use warpium_emulator::test_support::{FakeEmulatorConfig, FakeEmulatorThread};
    use warpium_emulator::{GrpcToken, RunningEmulator};
    use warpui::keymap::EditableBinding;
    use warpui::{AppContext, SingletonEntity as _};

    use super::DevicesPaneManager;
    use crate::warpium::FakeEmulatorKind;
    use crate::workspace::WorkspaceAction;

    /// Fake console ports start here, away from real emulators' 5554 and up.
    const FIRST_CONSOLE_PORT: u16 = 5900;
    const FAKE_TOKEN: &str = "warpium-debug-token";
    /// About 30 frames per second, like a guest animating.
    const FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

    #[derive(Default)]
    pub(crate) struct FakeEmulators {
        running: Vec<(FakeEmulatorThread, RunningEmulator)>,
    }

    impl FakeEmulators {
        pub(super) fn entries(&self) -> Vec<RunningEmulator> {
            self.running
                .iter()
                .map(|(_, entry)| entry.clone())
                .collect()
        }
    }

    pub(super) fn init_bindings(ctx: &mut AppContext) {
        use warpui::keymap::macros::*;

        let fake_binding =
            |name: &'static str, description: &'static str, action: WorkspaceAction| {
                EditableBinding::new(name, description, action)
                    .with_context_predicate(id!("Workspace"))
                    .with_enabled(|| {
                        warp_core::features::FeatureFlag::WarpiumEmbeddedEmulator.is_enabled()
                    })
            };
        ctx.register_editable_bindings([
            fake_binding(
                "workspace:debug_start_fake_emulator",
                "[Debug] Start fake Android emulator",
                WorkspaceAction::StartFakeAndroidEmulator(FakeEmulatorKind::Working),
            ),
            fake_binding(
                "workspace:debug_start_fake_emulator_stale_token",
                "[Debug] Start fake Android emulator with a stale token",
                WorkspaceAction::StartFakeAndroidEmulator(FakeEmulatorKind::StaleToken),
            ),
            fake_binding(
                "workspace:debug_start_fake_emulator_tls",
                "[Debug] Start fake Android emulator that requires TLS",
                WorkspaceAction::StartFakeAndroidEmulator(FakeEmulatorKind::TlsOnly),
            ),
            fake_binding(
                "workspace:debug_stop_fake_emulators",
                "[Debug] Stop fake Android emulators",
                WorkspaceAction::StopFakeAndroidEmulators,
            ),
        ]);
    }

    /// Starts a fake and returns its adb serial.
    pub(crate) fn start(kind: FakeEmulatorKind, ctx: &mut AppContext) -> Option<String> {
        DevicesPaneManager::handle(ctx).update(ctx, |manager, _| {
            let console_port = FIRST_CONSOLE_PORT + 2 * manager.fakes.running.len() as u16;
            let config = FakeEmulatorConfig {
                token: Some(FAKE_TOKEN.to_owned()),
                display: DisplaySize {
                    width: 1080,
                    height: 2400,
                },
                frames_per_stream: u32::MAX,
                frame_interval: Some(FRAME_INTERVAL),
                ..Default::default()
            };
            let fake = match FakeEmulatorThread::spawn(config, console_port) {
                Ok(fake) => fake,
                Err(err) => {
                    log::error!("[Devices] Couldn't start a fake emulator: {err}");
                    return None;
                }
            };
            let mut entry = fake.entry().clone();
            match kind {
                FakeEmulatorKind::Working => {}
                FakeEmulatorKind::StaleToken => {
                    entry.grpc_token = Some(GrpcToken::new("stale-token"));
                }
                FakeEmulatorKind::TlsOnly => entry.grpc_uses_tls = true,
            }
            log::info!(
                "[Devices] Started fake emulator {} ({kind:?}) on gRPC port {}",
                entry.adb_serial(),
                entry.grpc_port
            );
            let serial = entry.adb_serial();
            manager.fakes.running.push((fake, entry));
            Some(serial)
        })
    }

    pub(crate) fn stop_all(ctx: &mut AppContext) {
        DevicesPaneManager::handle(ctx).update(ctx, |manager, _| {
            log::info!(
                "[Devices] Stopping {} fake emulator(s)",
                manager.fakes.running.len()
            );
            manager.fakes.running.clear();
        });
    }
}

#[cfg(test)]
#[path = "emulator_tests.rs"]
mod tests;
