//! Warpium: the per-project toolbar in the top-right corner of the window, JetBrains-style.
//!
//! The toolbar shows the buttons of the active tab's project: the team config in a committed
//! `.warpium/toolbar.toml` merged with the user's overrides in
//! `~/.warp-oss/projects/<project-id>/toolbar.toml`. Buttons marked `run_config` and the
//! project's launch configurations go in a run-configurations dropdown with a run button; the
//! rest are icon buttons, and those that don't fit the title bar go in an overflow menu. The
//! config format, merge, detection and trust rules live in the `warpium_project_toolbar` crate;
//! this module supplies the view, the file watching and the persisted state. The workspace runs
//! the actions.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::channel::oneshot;
use notify_debouncer_full::notify::{RecursiveMode, WatchFilter};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::color::internal_colors;
use warp_core::user_preferences::GetUserPreferences as _;
use warpium_android::project::{AndroidAppModule, find_app_modules};
use warpium_project_toolbar::{
    ButtonIcon, ConfigError, ConfigSource, EditorModel, Placement, ProjectContext, RunConfigItem,
    RunConfigKind, SavePlan, TextPosition, ToolbarButton, ToolbarUserState, TrustChange,
    TrustListing, TrustReview, android_app_run_config, content_hash, detect_project,
    inline_buttons, merge, needs_trust, overflow_menu_buttons, parse_config, personal_config_path,
    run_config_items, selected_run_config, team_config_path, trust_review,
};
use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{
    Border, CacheOption, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable,
    ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Element, Empty, Fill, Flex,
    Icon as WarpUiIcon, Image, MouseStateHandle, OffsetPositioning, ParentAnchor, ParentElement,
    ParentOffsetBounds, Radius, Rect, ScrollbarWidth, Shrinkable, Stack, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::geometry::vector::vec2f;
use warpui::keymap::EditableBinding;
use warpui::scene::DropShadow;
use warpui::ui_components::button::{Button, ButtonVariant};
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Entity, ModelContext, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle,
};
use watcher::{BulkFilesystemWatcher, BulkFilesystemWatcherEvent};

use super::android::{AndroidEvent, AndroidModel, device_menu_items};
use super::is_warpium_channel;
use super::project_toolbar_row::ToolbarRow;
use super::sync_settings::{
    PROJECT_TOOLBAR_CONTEXT_FLAG, WarpiumSettings, WarpiumSettingsChangedEvent,
};
use crate::launch_configs::launch_config::PaneTemplateType;
use crate::menu::{Event as MenuEvent, Menu, MenuItem, MenuItemFields};
use crate::settings_view::ToggleSettingActionPair;
use crate::user_config::{WarpConfig, WarpConfigUpdateEvent};
use crate::util::bindings::BindingGroup;
use crate::view_components::DismissibleToast;
use crate::workspace::{ToastStack, WorkspaceAction};

/// Private-preferences key holding the serialized [`ToolbarUserState`].
const STATE_KEY: &str = "WarpiumProjectToolbarState";
const WATCHER_DEBOUNCE: Duration = Duration::from_millis(300);
const BUTTON_HEIGHT: f32 = 24.;
const ICON_SIZE: f32 = 16.;
const FONT_SIZE: f32 = 12.;
const RUN_CHIP_MAX_LABEL_WIDTH: f32 = 160.;
const DEVICE_CHIP_MAX_LABEL_WIDTH: f32 = 140.;
const MENU_WIDTH: f32 = 240.;
const TRUST_PROMPT_WIDTH: f32 = 420.;
/// The trust prompt's command list scrolls past this height, keeping its buttons in view.
const TRUST_LIST_MAX_HEIGHT: f32 = 320.;

/// Holds the toolbar state shared by all windows: trust and run-configuration selections
/// (persisted in private preferences, never in the project) and the file watcher.
pub(crate) struct ProjectToolbarStore {
    state: ToolbarUserState,
    watcher: Option<ModelHandle<BulkFilesystemWatcher>>,
    /// Directories being watched, with how many toolbars watch each.
    watched: HashMap<PathBuf, usize>,
}

pub(crate) enum ProjectToolbarStoreEvent {
    StateChanged,
    FilesChanged(Vec<PathBuf>),
}

impl Entity for ProjectToolbarStore {
    type Event = ProjectToolbarStoreEvent;
}

impl SingletonEntity for ProjectToolbarStore {}

/// Registers the shared toolbar store, once.
fn ensure_store(ctx: &mut AppContext) {
    if ctx.has_singleton_model::<ProjectToolbarStore>() {
        return;
    }
    ctx.add_singleton_model(|ctx: &mut ModelContext<ProjectToolbarStore>| {
        let state = ctx
            .private_user_preferences()
            .read_value(STATE_KEY)
            .ok()
            .flatten()
            .map(|json| ToolbarUserState::from_json(&json))
            .unwrap_or_default();
        ProjectToolbarStore {
            state,
            watcher: None,
            watched: HashMap::new(),
        }
    });
}

/// Registers the command palette actions for the project toolbar.
pub(crate) fn init_bindings(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_editable_bindings([
        EditableBinding::new(
            "workspace:customize_project_toolbar",
            "Customize project toolbar",
            WorkspaceAction::CustomizeProjectToolbar,
        )
        .with_group(BindingGroup::Settings.as_str())
        .with_context_predicate(id!("Workspace")),
        EditableBinding::new(
            "workspace:edit_project_toolbar",
            "Edit project toolbar",
            WorkspaceAction::EditProjectToolbar,
        )
        .with_group(BindingGroup::Settings.as_str())
        .with_context_predicate(id!("Workspace")),
        EditableBinding::new(
            "workspace:edit_personal_toolbar_overrides",
            "Edit personal toolbar overrides",
            WorkspaceAction::EditPersonalToolbarOverrides,
        )
        .with_group(BindingGroup::Settings.as_str())
        .with_context_predicate(id!("Workspace")),
    ]);

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "project toolbar",
            WorkspaceAction::ToggleProjectToolbar,
            &id!("Workspace"),
            PROJECT_TOOLBAR_CONTEXT_FLAG,
        )],
        app,
    );
    super::project_toolbar_editor::init(app);
    super::project_toolbar_prompt::init(app);
}

impl ProjectToolbarStore {
    fn state(&self) -> &ToolbarUserState {
        &self.state
    }

    fn update_state(
        &mut self,
        update: impl FnOnce(&mut ToolbarUserState),
        ctx: &mut ModelContext<Self>,
    ) {
        update(&mut self.state);
        if let Err(err) = ctx
            .private_user_preferences()
            .write_value(STATE_KEY, self.state.to_json())
        {
            log::warn!("[Project toolbar] Failed to save the toolbar state: {err:#}");
        }
        ctx.emit(ProjectToolbarStoreEvent::StateChanged);
    }

    /// Trusts a team config the user wrote from the toolbar editor, when the version it replaced
    /// was trusted (see [`ToolbarUserState::record_gui_save`]).
    fn record_gui_save(
        &mut self,
        root: &Path,
        previous_team_text: Option<&str>,
        new_team_text: &str,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_state(
            |state| {
                state.record_gui_save(root, previous_team_text, new_team_text);
            },
            ctx,
        );
    }

    fn watch(&mut self, dir: &Path, ctx: &mut ModelContext<Self>) {
        let count = self.watched.entry(dir.to_path_buf()).or_default();
        *count += 1;
        if *count > 1 {
            return;
        }
        let watcher = self.watcher.get_or_insert_with(|| {
            let watcher = ctx.add_model(|ctx| BulkFilesystemWatcher::new(WATCHER_DEBOUNCE, ctx));
            ctx.subscribe_to_model(&watcher, |_, _, event, ctx| {
                Self::handle_fs_event(event, ctx);
            });
            watcher
        });
        let registration = watcher.update(ctx, |watcher, _| {
            watcher.register_path(dir, WatchFilter::accept_all(), RecursiveMode::NonRecursive)
        });
        ctx.spawn(registration, |_, result, _| {
            if let Err(err) = result {
                log::warn!("[Project toolbar] Failed to watch a toolbar directory: {err:#}");
            }
        });
    }

    fn unwatch(&mut self, dir: &Path, ctx: &mut ModelContext<Self>) {
        let Some(count) = self.watched.get_mut(dir) else {
            return;
        };
        *count -= 1;
        if *count > 0 {
            return;
        }
        self.watched.remove(dir);
        if let Some(watcher) = &self.watcher {
            let unregistration = watcher.update(ctx, |watcher, _| watcher.unregister_path(dir));
            ctx.spawn(unregistration, |_, result, _| {
                if let Err(err) = result {
                    log::debug!("[Project toolbar] Failed to stop watching a directory: {err:#}");
                }
            });
        }
    }

    fn handle_fs_event(event: &BulkFilesystemWatcherEvent, ctx: &mut ModelContext<Self>) {
        let paths = event
            .added_or_updated_iter()
            .chain(event.deleted.iter())
            .chain(event.moved.keys())
            .chain(event.moved.values())
            .cloned()
            .collect::<Vec<_>>();
        if !paths.is_empty() {
            ctx.emit(ProjectToolbarStoreEvent::FilesChanged(paths));
        }
    }
}

/// Whether the project toolbar is shown at all.
pub(crate) fn is_enabled(ctx: &AppContext) -> bool {
    is_warpium_channel() && *WarpiumSettings::as_ref(ctx).project_toolbar
}

/// The project of a tab with this pinned Project Explorer root and location; an
/// [`ProjectLocation::Unknown`] location counts as no working directory.
fn detect_tab_project(
    pinned_root: Option<PathBuf>,
    location: ProjectLocation,
) -> Option<ProjectContext> {
    let canonical = |path: PathBuf| dunce::canonicalize(&path).unwrap_or(path);
    let dir = match location {
        ProjectLocation::Directory(dir) => Some(canonical(dir)),
        ProjectLocation::NoProject | ProjectLocation::Unknown => None,
    };
    detect_project(pinned_root.map(canonical).as_deref(), dir.as_deref())
}

/// The personal overrides file of a project, when the home config directory is known.
pub(crate) fn personal_path(root: &Path) -> Option<PathBuf> {
    warp_core::paths::warp_home_config_dir().map(|home| personal_config_path(&home, root))
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProjectToolbarAction {
    ClickButton(String),
    ToggleRunConfigMenu,
    ToggleOverflowMenu,
    SelectRunConfig(String),
    RunSelectedRunConfig,
    OpenInvalidConfig,
    ConfirmTrust,
    CancelTrust,
    EditConfig {
        personal: bool,
    },
    ToggleDeviceMenu,
    SelectAndroidDevice(String),
    LaunchAndroidAvd(String),
    OpenLogcat,
    /// Open the toolbar editor, with this button selected.
    Customize(Option<String>),
    /// Open the toolbar editor with a new button.
    AddButton,
    /// Show a toolbar button's context menu.
    OpenButtonMenu(String),
    QuickEdit {
        id: String,
        edit: QuickEdit,
    },
}

/// A change made from a toolbar button's context menu, saved right away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuickEdit {
    Duplicate,
    MoveLeft,
    MoveRight,
    Hide,
    Delete,
}

pub(crate) enum ProjectToolbarEvent {
    /// Run a button's action for the project at `root`. Trust was already checked.
    Run {
        root: PathBuf,
        button: ToolbarButton,
    },
    OpenLaunchConfig(String),
    /// Open the team (or personal) config of the project, creating it from the template.
    EditConfig {
        personal: bool,
    },
    /// Open a config file, at the problem in it when known.
    OpenFile {
        path: PathBuf,
        location: Option<TextPosition>,
    },
    /// The toolbar's menu closed, so focus can go back to the session.
    MenuClosed,
    /// [`ProjectToolbarView::has_content`] changed, so its title-bar slot appears or goes away.
    ContentChanged,
    /// Build, install and start the Android app module with this Gradle path on the selected
    /// device.
    RunAndroidApp {
        root: PathBuf,
        module: AndroidAppModule,
    },
    OpenLogcat,
    /// Open the toolbar editor for the project at `root`, with the button `select` selected or,
    /// with `add`, a new button.
    Customize {
        root: PathBuf,
        select: Option<String>,
        add: bool,
    },
}

enum LoadedConfig {
    Buttons {
        buttons: Vec<ToolbarButton>,
        /// The [`content_hash`] of the team file, when there is one.
        team_hash: Option<String>,
        /// The team file's buttons without the personal overrides: what trusting it covers.
        team_buttons: Vec<ToolbarButton>,
    },
    Invalid {
        error: ConfigError,
        path: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenMenu {
    RunConfigs,
    Overflow,
    Devices,
    /// The context menu of the button in [`ProjectToolbarView::context_button`].
    Button,
}

struct TrustPrompt {
    target: TrustTarget,
    review: TrustReview,
    list_scroll: ClippedScrollStateHandle,
}

/// What a trust prompt asks to trust, and what runs once it is confirmed.
enum TrustTarget {
    /// One version of the team config; the button runs afterwards.
    TeamConfig {
        pending_button_id: String,
        team_hash: String,
    },
    /// The project's Gradle build, for Android runs; the app module runs afterwards.
    AndroidBuild { module: AndroidAppModule },
}

#[derive(Default)]
struct MouseStates {
    run_config_chip: MouseStateHandle,
    device_chip: MouseStateHandle,
    run_button: MouseStateHandle,
    overflow_button: MouseStateHandle,
    add_button: MouseStateHandle,
    invalid_config_button: MouseStateHandle,
    trust_confirm: MouseStateHandle,
    trust_cancel: MouseStateHandle,
}

/// Where the active tab is working, besides its pinned Project Explorer root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectLocation {
    /// The folder of the focused editor's file, or the working directory of a local session.
    Directory(PathBuf),
    /// A remote session, which has no project.
    NoProject,
    /// Nothing to go by, such as a tab showing only settings: the toolbar keeps its project.
    Unknown,
}

/// The toolbar of one window, following its active tab.
pub(crate) struct ProjectToolbarView {
    /// The last pinned explorer root and location, so the toolbar can be recomputed when it is
    /// turned back on.
    inputs: (Option<PathBuf>, ProjectLocation),
    project: Option<ProjectContext>,
    /// The project last detected from a tab's inputs, even while the toolbar is off, which a tab
    /// with an unknown location keeps.
    last_detected: Option<ProjectContext>,
    loaded: Option<LoadedConfig>,
    run_configs: Vec<RunConfigItem>,
    watched_dirs: Vec<PathBuf>,
    menu: ViewHandle<Menu<ProjectToolbarAction>>,
    open_menu: Option<OpenMenu>,
    /// The button whose context menu is open.
    context_button: Option<String>,
    trust_prompt: Option<TrustPrompt>,
    /// Per button id, kept across reloads.
    button_mouse_states: HashMap<String, MouseStateHandle>,
    mouse_states: MouseStates,
    /// How many toolbar buttons fit inline at the last layout; the overflow menu lists the rest.
    visible_buttons: Arc<AtomicUsize>,
    /// [`Self::has_content`] when it was last reported to the workspace.
    had_content: bool,
    /// The project's Android application modules, found off the main thread. When there are
    /// any, the toolbar shows the device picker and one run configuration per module.
    android_apps: Vec<AndroidAppModule>,
    /// Bumped per detection, so results for an earlier project are dropped.
    android_generation: u64,
    /// The Gradle path of an Android run waiting for an emulator to finish booting.
    pending_android_run: Option<String>,
}

impl ProjectToolbarView {
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        ensure_store(ctx);
        let menu = ctx.add_typed_action_view(|_| {
            Menu::new()
                .with_width(MENU_WIDTH)
                .with_drop_shadow()
                .prevent_interaction_with_other_elements()
        });
        ctx.subscribe_to_view(&menu, |me, _, event, ctx| match event {
            MenuEvent::Close { .. } => {
                me.open_menu = None;
                ctx.emit(ProjectToolbarEvent::MenuClosed);
                ctx.notify();
            }
            MenuEvent::ItemSelected | MenuEvent::ItemHovered => {}
        });
        ctx.subscribe_to_model(
            &ProjectToolbarStore::handle(ctx),
            |me, _, event, ctx| match event {
                ProjectToolbarStoreEvent::StateChanged => ctx.notify(),
                ProjectToolbarStoreEvent::FilesChanged(paths) => {
                    if me.is_relevant_change(paths) {
                        me.reload(ctx);
                    }
                }
            },
        );
        ctx.subscribe_to_model(
            &WarpiumSettings::handle(ctx),
            |me, _, event, ctx| match event {
                WarpiumSettingsChangedEvent::ProjectToolbar { .. } => {
                    let (pinned_root, location) = me.inputs.clone();
                    me.update_context(pinned_root, location, true, ctx);
                }
                WarpiumSettingsChangedEvent::AndroidTools { .. } => me.detect_android_apps(ctx),
                WarpiumSettingsChangedEvent::SyncFromWarp { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheck { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheckIntervalHours { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheckMinCommits { .. }
                | WarpiumSettingsChangedEvent::UpdateCheck { .. }
                | WarpiumSettingsChangedEvent::UpdateCheckIntervalHours { .. }
                | WarpiumSettingsChangedEvent::CodeIndex { .. } => {}
            },
        );
        if ctx.has_singleton_model::<AndroidModel>() {
            ctx.subscribe_to_model(&AndroidModel::handle(ctx), |me, _, event, ctx| {
                match event {
                    // A pending run goes to whichever device is selected and online first: the
                    // booted emulator, or another device the user picked meanwhile.
                    AndroidEvent::AvdBooted
                    | AndroidEvent::SelectionChanged
                    | AndroidEvent::DevicesChanged => {
                        let android = AndroidModel::as_ref(ctx);
                        let ready = android
                            .selected_device()
                            .is_some_and(|device| device.entry.state.is_online());
                        if ready && let Some(gradle_path) = me.pending_android_run.take() {
                            me.run_android_app(gradle_path, ctx);
                        }
                    }
                    AndroidEvent::AvdLaunchFailed => {
                        if !AndroidModel::as_ref(ctx).is_any_launching() {
                            me.pending_android_run = None;
                        }
                    }
                    AndroidEvent::AvdsChanged | AndroidEvent::AppLaunched(_) => {}
                }
                if me.open_menu == Some(OpenMenu::Devices) {
                    me.set_menu_items(OpenMenu::Devices, ctx);
                }
                ctx.notify();
            });
        }
        ctx.subscribe_to_model(&WarpConfig::handle(ctx), |me, _, event, ctx| {
            if let WarpConfigUpdateEvent::LaunchConfigs = event {
                me.refresh_run_configs(ctx);
            }
        });
        Self {
            inputs: (None, ProjectLocation::Unknown),
            project: None,
            last_detected: None,
            loaded: None,
            run_configs: Vec::new(),
            watched_dirs: Vec::new(),
            menu,
            open_menu: None,
            context_button: None,
            trust_prompt: None,
            button_mouse_states: HashMap::new(),
            mouse_states: MouseStates::default(),
            visible_buttons: Arc::new(AtomicUsize::new(usize::MAX)),
            had_content: false,
            android_apps: Vec::new(),
            android_generation: 0,
            pending_android_run: None,
        }
    }

    /// Whether the toolbar shows anything: buttons, the run-configurations widget, an invalid
    /// config warning, or for a project without buttons the "Add button" affordance.
    pub(crate) fn has_content(&self) -> bool {
        self.project.is_some() && self.loaded.is_some()
    }

    /// Whether running `button` (such as an unsaved one from the editor) in the project at
    /// `root` must wait until the user trusts the team config: a team command of a project other
    /// than the toolbar's, or of an untrusted version.
    pub(crate) fn needs_trust_to_run(
        &self,
        root: &Path,
        button: &ToolbarButton,
        ctx: &AppContext,
    ) -> bool {
        if !button.requires_trust() {
            return false;
        }
        self.project_root() != Some(root)
            || needs_trust(
                button,
                root,
                self.team_hash(),
                ProjectToolbarStore::as_ref(ctx).state(),
            )
    }

    /// The detected project, if the toolbar is on.
    pub(crate) fn project_root(&self) -> Option<&Path> {
        self.project.as_ref().map(|project| project.root.as_path())
    }

    /// The project of the tab it last followed, whether or not the toolbar is on.
    pub(crate) fn detected_project_root(&self) -> Option<&Path> {
        self.last_detected
            .as_ref()
            .map(|project| project.root.as_path())
    }

    /// Follows the active tab: its pinned Project Explorer root and where its focused pane is.
    /// `force` detects the project again even when these are unchanged.
    pub(crate) fn set_context(
        &mut self,
        pinned_root: Option<PathBuf>,
        location: ProjectLocation,
        force: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        self.update_context(pinned_root, location, force, ctx);
    }

    fn update_context(
        &mut self,
        pinned_root: Option<PathBuf>,
        location: ProjectLocation,
        force: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let inputs = (pinned_root, location);
        if !force && self.inputs == inputs {
            return;
        }
        self.inputs = inputs.clone();
        let (pinned_root, location) = inputs;
        if pinned_root.is_some() || location != ProjectLocation::Unknown {
            self.last_detected = detect_tab_project(pinned_root, location);
        }
        let project = is_enabled(ctx)
            .then(|| self.last_detected.clone())
            .flatten();
        if project == self.project {
            return;
        }
        self.project = project;
        self.open_menu = None;
        self.trust_prompt = None;
        self.reload(ctx);
        self.detect_android_apps(ctx);
    }

    /// Finds the project's Android application modules off the main thread, and starts device
    /// tracking when there are any.
    fn detect_android_apps(&mut self, ctx: &mut ViewContext<Self>) {
        self.android_generation += 1;
        self.android_apps.clear();
        self.pending_android_run = None;
        self.refresh_run_configs(ctx);
        let Some(root) = self.project_root().map(Path::to_path_buf) else {
            return;
        };
        if !super::android::is_enabled(ctx) {
            return;
        }
        let generation = self.android_generation;
        let (sender, receiver) = oneshot::channel();
        let thread_root = root.clone();
        let spawned = std::thread::Builder::new()
            .name("android-modules".to_owned())
            .spawn(move || {
                let _ = sender.send(find_app_modules(&thread_root));
            });
        if let Err(err) = spawned {
            log::warn!("[Project toolbar] Couldn't start looking for Android modules: {err}");
            return;
        }
        ctx.spawn(async move { receiver.await.ok() }, move |me, apps, ctx| {
            if me.android_generation != generation {
                return;
            }
            me.android_apps = apps.unwrap_or_default();
            if !me.android_apps.is_empty() {
                AndroidModel::handle(ctx).update(ctx, |android, ctx| {
                    android.ensure_started(Some(root.as_path()), ctx)
                });
            }
            me.refresh_run_configs(ctx);
        });
    }

    /// Whether the toolbar shows the device picker: an Android project with the tools on.
    fn shows_android(&self, ctx: &AppContext) -> bool {
        !self.android_apps.is_empty() && super::android::is_enabled(ctx)
    }

    /// Asks the workspace to run an Android app module, or waits for an emulator that is
    /// booting.
    fn run_android_app(&mut self, gradle_path: String, ctx: &mut ViewContext<Self>) {
        let (Some(root), Some(module)) = (
            self.project_root().map(Path::to_path_buf),
            self.android_apps
                .iter()
                .find(|module| module.gradle_path == gradle_path)
                .cloned(),
        ) else {
            return;
        };
        if !super::android::is_enabled(ctx) {
            return;
        }
        if !ProjectToolbarStore::as_ref(ctx)
            .state()
            .trusts_android_builds(&root)
        {
            let gradle = if root.join("gradlew").is_file() {
                "./gradlew"
            } else {
                "gradle"
            };
            self.trust_prompt = Some(TrustPrompt {
                review: TrustReview {
                    listings: vec![TrustListing {
                        label: module.display_name(),
                        command: format!("{gradle} {}", module.install_task("debug")),
                        details: "current session · env ANDROID_SERIAL".to_owned(),
                        change: TrustChange::New,
                    }],
                    is_update: false,
                    removed: 0,
                },
                target: TrustTarget::AndroidBuild { module },
                list_scroll: ClippedScrollStateHandle::default(),
            });
            ctx.notify();
            return;
        }
        let android = AndroidModel::as_ref(ctx);
        let device_online = android
            .selected_device()
            .is_some_and(|device| device.entry.state.is_online());
        if !device_online && android.is_any_launching() {
            self.pending_android_run = Some(gradle_path);
            ctx.notify();
            return;
        }
        self.pending_android_run = None;
        ctx.emit(ProjectToolbarEvent::RunAndroidApp { root, module });
        ctx.notify();
    }

    /// Opens the device picker's menu, as when a run needs a device.
    pub(crate) fn open_device_menu(&mut self, ctx: &mut ViewContext<Self>) {
        if self.shows_android(ctx) && self.open_menu != Some(OpenMenu::Devices) {
            self.toggle_menu(OpenMenu::Devices, ctx);
        }
    }

    /// Reads the configs again, for example after "Edit personal toolbar overrides" created one.
    pub(crate) fn reload(&mut self, ctx: &mut ViewContext<Self>) {
        self.loaded = self.project.as_ref().map(|project| load(&project.root));
        if let Some(LoadedConfig::Invalid { error, .. }) = &self.loaded {
            warp_core::safe_warn!(
                safe: ("[Project toolbar] The toolbar config is invalid"),
                full: ("[Project toolbar] The toolbar config is invalid: {error}")
            );
        }
        if let Some(LoadedConfig::Buttons { buttons, .. }) = &self.loaded {
            for button in buttons {
                self.button_mouse_states
                    .entry(button.id.clone())
                    .or_default();
            }
        }
        if let Some(TrustPrompt {
            target: TrustTarget::TeamConfig {
                pending_button_id, ..
            },
            ..
        }) = &self.trust_prompt
            && self.button(pending_button_id).is_none()
        {
            self.trust_prompt = None;
        }
        self.sync_watched_dirs(ctx);
        self.refresh_run_configs(ctx);
        ctx.notify();
    }

    fn buttons(&self) -> &[ToolbarButton] {
        match &self.loaded {
            Some(LoadedConfig::Buttons { buttons, .. }) => buttons,
            Some(LoadedConfig::Invalid { .. }) | None => &[],
        }
    }

    fn button(&self, id: &str) -> Option<&ToolbarButton> {
        self.buttons().iter().find(|button| button.id == id)
    }

    fn team_buttons(&self) -> &[ToolbarButton] {
        match &self.loaded {
            Some(LoadedConfig::Buttons { team_buttons, .. }) => team_buttons,
            Some(LoadedConfig::Invalid { .. }) | None => &[],
        }
    }

    fn team_hash(&self) -> Option<&str> {
        match &self.loaded {
            Some(LoadedConfig::Buttons { team_hash, .. }) => team_hash.as_deref(),
            Some(LoadedConfig::Invalid { .. }) | None => None,
        }
    }

    fn is_relevant_change(&self, paths: &[PathBuf]) -> bool {
        let Some(root) = self.project_root() else {
            return false;
        };
        let team_dir = root.join(warpium_project_toolbar::PROJECT_CONFIG_DIR);
        let personal_dir =
            personal_path(root).and_then(|path| path.parent().map(Path::to_path_buf));
        paths.iter().any(|path| {
            path.starts_with(&team_dir)
                || personal_dir
                    .as_ref()
                    .is_some_and(|personal_dir| path.starts_with(personal_dir))
        })
    }

    /// Watches the project root (to notice `.warpium/` appearing), `.warpium/` and the personal
    /// directory, whichever exist.
    fn sync_watched_dirs(&mut self, ctx: &mut ViewContext<Self>) {
        let desired = self
            .project_root()
            .map(|root| {
                let mut dirs = vec![
                    root.to_path_buf(),
                    root.join(warpium_project_toolbar::PROJECT_CONFIG_DIR),
                ];
                if let Some(personal_dir) =
                    personal_path(root).and_then(|path| path.parent().map(Path::to_path_buf))
                {
                    dirs.push(personal_dir);
                }
                dirs.retain(|dir| dir.is_dir());
                dirs
            })
            .unwrap_or_default();
        if desired == self.watched_dirs {
            return;
        }
        let previous = std::mem::replace(&mut self.watched_dirs, desired.clone());
        ProjectToolbarStore::handle(ctx).update(ctx, |store, ctx| {
            for dir in desired.iter().filter(|dir| !previous.contains(dir)) {
                store.watch(dir, ctx);
            }
            for dir in previous.iter().filter(|dir| !desired.contains(dir)) {
                store.unwatch(dir, ctx);
            }
        });
    }

    fn refresh_run_configs(&mut self, ctx: &mut ViewContext<Self>) {
        let launch_configs = self
            .project_root()
            .map(|root| project_launch_configs(root, ctx))
            .unwrap_or_default();
        self.run_configs = run_config_items(self.buttons(), &launch_configs);
        if super::android::is_enabled(ctx) {
            let android_items: Vec<RunConfigItem> = self
                .android_apps
                .iter()
                .map(|module| android_app_run_config(&module.gradle_path, &module.display_name()))
                .collect();
            self.run_configs.extend(android_items);
        }
        let has_content = self.has_content();
        if has_content != self.had_content {
            self.had_content = has_content;
            ctx.emit(ProjectToolbarEvent::ContentChanged);
        }
        ctx.notify();
    }

    fn selected_run_config<'a>(&'a self, ctx: &AppContext) -> Option<&'a RunConfigItem> {
        let remembered = self.project_root().and_then(|root| {
            ProjectToolbarStore::as_ref(ctx)
                .state()
                .selected_run_config(root)
        });
        selected_run_config(&self.run_configs, remembered)
    }

    fn click_button(&mut self, id: &str, ctx: &mut ViewContext<Self>) {
        self.open_menu = None;
        let (Some(root), Some(button)) = (
            self.project_root().map(Path::to_path_buf),
            self.button(id).cloned(),
        ) else {
            return;
        };
        let team_hash = self.team_hash().map(str::to_owned);
        let state = ProjectToolbarStore::as_ref(ctx).state();
        if needs_trust(&button, &root, team_hash.as_deref(), state) {
            // Without a hash there is no version of the team file the user could trust.
            if let Some(team_hash) = team_hash {
                self.trust_prompt = Some(TrustPrompt {
                    target: TrustTarget::TeamConfig {
                        pending_button_id: id.to_owned(),
                        team_hash,
                    },
                    review: trust_review(self.team_buttons(), state.trusted_commands(&root)),
                    list_scroll: ClippedScrollStateHandle::default(),
                });
            }
            ctx.notify();
            return;
        }
        ctx.emit(ProjectToolbarEvent::Run { root, button });
        ctx.notify();
    }

    fn confirm_trust(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(prompt) = self.trust_prompt.take() else {
            return;
        };
        let Some(root) = self.project_root().map(Path::to_path_buf) else {
            return;
        };
        match prompt.target {
            TrustTarget::TeamConfig {
                pending_button_id,
                team_hash,
            } => {
                // The file may have changed while the prompt was open; the listed commands are
                // only trusted for the version they came from.
                if self.team_hash() != Some(team_hash.as_str()) {
                    ctx.notify();
                    return;
                }
                let team_buttons = self.team_buttons();
                ProjectToolbarStore::handle(ctx).update(ctx, |store, ctx| {
                    store.update_state(|state| state.trust(&root, &team_hash, team_buttons), ctx);
                });
                self.click_button(&pending_button_id, ctx);
            }
            TrustTarget::AndroidBuild { module } => {
                ProjectToolbarStore::handle(ctx).update(ctx, |store, ctx| {
                    store.update_state(|state| state.trust_android_builds(&root), ctx);
                });
                self.run_android_app(module.gradle_path, ctx);
            }
        }
    }

    fn run_selected_run_config(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(item) = self.selected_run_config(ctx).cloned() else {
            return;
        };
        match item.kind {
            RunConfigKind::Button(id) => self.click_button(&id, ctx),
            RunConfigKind::LaunchConfig(name) => {
                self.open_menu = None;
                ctx.emit(ProjectToolbarEvent::OpenLaunchConfig(name));
            }
            RunConfigKind::AndroidApp(gradle_path) => {
                self.open_menu = None;
                self.run_android_app(gradle_path, ctx);
            }
        }
    }

    fn toggle_menu(&mut self, menu: OpenMenu, ctx: &mut ViewContext<Self>) {
        if self.open_menu == Some(menu) {
            self.open_menu = None;
            ctx.notify();
            return;
        }
        self.trust_prompt = None;
        match menu {
            OpenMenu::RunConfigs => self.refresh_run_configs(ctx),
            OpenMenu::Devices => {
                let root = self.project_root().map(Path::to_path_buf);
                AndroidModel::handle(ctx).update(ctx, |android, ctx| {
                    android.ensure_started(root.as_deref(), ctx);
                    android.refresh_avds(ctx);
                });
            }
            OpenMenu::Overflow | OpenMenu::Button => {}
        }
        self.set_menu_items(menu, ctx);
        self.open_menu = Some(menu);
        ctx.focus(&self.menu);
        ctx.notify();
    }

    fn set_menu_items(&mut self, menu: OpenMenu, ctx: &mut ViewContext<Self>) {
        let items = match menu {
            OpenMenu::RunConfigs => self.run_config_menu_items(),
            OpenMenu::Overflow => self.overflow_menu_items(),
            OpenMenu::Button => self.button_menu_items(),
            OpenMenu::Devices => {
                let mut items = device_menu_items(
                    AndroidModel::as_ref(ctx),
                    ProjectToolbarAction::SelectAndroidDevice,
                    ProjectToolbarAction::LaunchAndroidAvd,
                );
                items.push(MenuItem::Separator);
                items.push(MenuItem::Item(
                    MenuItemFields::new("Open Logcat")
                        .with_on_select_action(ProjectToolbarAction::OpenLogcat),
                ));
                items
            }
        };
        let selected = match menu {
            OpenMenu::RunConfigs => self
                .selected_run_config(ctx)
                .map(|item| ProjectToolbarAction::SelectRunConfig(item.key.clone())),
            OpenMenu::Devices => AndroidModel::as_ref(ctx)
                .selected_serial()
                .map(|serial| ProjectToolbarAction::SelectAndroidDevice(serial.to_owned())),
            OpenMenu::Overflow | OpenMenu::Button => None,
        };
        self.menu.update(ctx, |menu_view, ctx| {
            menu_view.set_items(items, ctx);
            if let Some(selected) = &selected {
                menu_view.set_selected_by_action(selected, ctx);
            }
        });
    }

    fn run_config_menu_items(&self) -> Vec<MenuItem<ProjectToolbarAction>> {
        let mut items = vec![MenuItem::Header {
            fields: MenuItemFields::new("Run configurations").with_no_interaction_on_hover(),
            clickable: false,
            right_side_fields: None,
        }];
        items.extend(self.run_configs.iter().map(|item| {
            let label = match item.kind {
                RunConfigKind::Button(_) => item.label.clone(),
                RunConfigKind::LaunchConfig(_) => format!("{} (launch configuration)", item.label),
                RunConfigKind::AndroidApp(_) => format!("{} (Android app)", item.label),
            };
            MenuItem::Item(
                MenuItemFields::new(label)
                    .with_on_select_action(ProjectToolbarAction::SelectRunConfig(item.key.clone())),
            )
        }));
        items.extend(edit_menu_items());
        items
    }

    fn overflow_menu_items(&self) -> Vec<MenuItem<ProjectToolbarAction>> {
        let mut items =
            overflow_menu_buttons(self.buttons(), self.visible_buttons.load(Ordering::Relaxed))
                .into_iter()
                .map(|button| {
                    MenuItem::Item(
                        MenuItemFields::new(button.display_label())
                            .with_tooltip(button.tooltip_text())
                            .with_on_select_action(ProjectToolbarAction::ClickButton(
                                button.id.clone(),
                            )),
                    )
                })
                .collect::<Vec<_>>();
        items.extend(edit_menu_items());
        items
    }

    fn button_menu_items(&self) -> Vec<MenuItem<ProjectToolbarAction>> {
        let Some(id) = self.context_button.clone() else {
            return Vec::new();
        };
        let quick = |label: &str, edit| {
            MenuItem::Item(MenuItemFields::new(label).with_on_select_action(
                ProjectToolbarAction::QuickEdit {
                    id: id.clone(),
                    edit,
                },
            ))
        };
        vec![
            MenuItem::Item(
                MenuItemFields::new("Edit…")
                    .with_on_select_action(ProjectToolbarAction::Customize(Some(id.clone()))),
            ),
            quick("Duplicate", QuickEdit::Duplicate),
            quick("Move left", QuickEdit::MoveLeft),
            quick("Move right", QuickEdit::MoveRight),
            MenuItem::Separator,
            quick("Hide", QuickEdit::Hide),
            quick("Delete", QuickEdit::Delete),
        ]
    }

    /// Applies a context-menu change to the config files and reloads.
    fn quick_edit(&mut self, id: &str, edit: QuickEdit, ctx: &mut ViewContext<Self>) {
        self.open_menu = None;
        let Some(root) = self.project_root().map(Path::to_path_buf) else {
            return;
        };
        match apply_quick_edit(&root, id, edit, ctx) {
            Ok(()) => self.reload(ctx),
            Err(message) => {
                let window_id = ctx.window_id();
                ToastStack::handle(ctx).update(ctx, |stack, ctx| {
                    stack.add_ephemeral_toast(
                        DismissibleToast::error(format!("Project toolbar: {message}")),
                        window_id,
                        ctx,
                    );
                });
            }
        }
        ctx.emit(ProjectToolbarEvent::MenuClosed);
        ctx.notify();
    }

    fn customize(&mut self, select: Option<String>, add: bool, ctx: &mut ViewContext<Self>) {
        self.open_menu = None;
        if let Some(root) = self.project_root().map(Path::to_path_buf) {
            ctx.emit(ProjectToolbarEvent::Customize { root, select, add });
        }
        ctx.notify();
    }

    /// The "+" at the end of the toolbar, or "+ Add button" when it has no buttons yet.
    fn render_add_button(&self, with_label: bool, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let color = theme.sub_text_color(theme.background());
        let plus = sized_icon(Icon::Plus.to_warpui_icon(color).finish());
        let label = if with_label {
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(plus)
                .with_child(
                    Container::new(
                        Text::new_inline("Add button", appearance.ui_font_family(), FONT_SIZE)
                            .with_color(color.into_solid())
                            .finish(),
                    )
                    .with_margin_left(2.)
                    .finish(),
                )
                .finish()
        } else {
            plus
        };
        let ui_builder = appearance.ui_builder().clone();
        toolbar_button(
            self.mouse_states.add_button.clone(),
            label,
            if with_label { (4., 6.) } else { (4., 4.) },
            appearance,
        )
        .with_tooltip(move || {
            ui_builder
                .tool_tip("Add a button to this project's toolbar".to_owned())
                .build()
                .finish()
        })
        .build()
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(ProjectToolbarAction::AddButton))
        .finish()
    }

    fn render_invalid_config(
        &self,
        error: &ConfigError,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let color = theme.ui_error_color();
        let label = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(sized_icon(
                Icon::AlertTriangle.to_warpui_icon(color.into()).finish(),
            ))
            .with_child(
                Container::new(
                    Text::new_inline("Toolbar config", appearance.ui_font_family(), FONT_SIZE)
                        .with_color(color)
                        .finish(),
                )
                .with_margin_left(4.)
                .finish(),
            )
            .finish();
        let ui_builder = appearance.ui_builder().clone();
        let heading = match error.source {
            ConfigSource::Team => "Invalid .warpium/toolbar.toml".to_owned(),
            ConfigSource::Personal => "Invalid personal toolbar overrides".to_owned(),
        };
        let detail = match error.location {
            Some(location) => format!(
                "{}\nClick to open the file at line {}.",
                error.message, location.line
            ),
            None => format!("{}\nClick to open the file.", error.message),
        };
        toolbar_button(
            self.mouse_states.invalid_config_button.clone(),
            label,
            (4., 6.),
            appearance,
        )
        .with_tooltip(move || {
            ui_builder
                .tool_tip_with_sublabel(heading, detail)
                .build()
                .finish()
        })
        .build()
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(ProjectToolbarAction::OpenInvalidConfig))
        .finish()
    }

    fn render_button(
        &self,
        button: &ToolbarButton,
        root: &Path,
        appearance: &Appearance,
        ctx: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let color = theme.sub_text_color(theme.background());
        let label = match &button.icon {
            Some(ButtonIcon::Bundled { asset_path, .. }) => {
                sized_icon(WarpUiIcon::new(asset_path, color.into_solid()).finish())
            }
            Some(ButtonIcon::Custom(path)) => sized_icon(
                Image::new(
                    AssetSource::LocalFile {
                        path: root
                            .join(warpium_project_toolbar::PROJECT_CONFIG_DIR)
                            .join(path)
                            .to_string_lossy()
                            .into_owned(),
                        content_version: None,
                    },
                    CacheOption::BySize,
                )
                .contain()
                .finish(),
            ),
            None => Text::new_inline(
                button.display_label().to_owned(),
                appearance.ui_font_family(),
                FONT_SIZE,
            )
            .with_color(color.into_solid())
            .finish(),
        };
        let padding = if button.icon.is_some() {
            (4., 4.)
        } else {
            (6., 6.)
        };
        let tooltip = button.tooltip_text();
        let sublabel = needs_trust(
            button,
            root,
            self.team_hash(),
            ProjectToolbarStore::as_ref(ctx).state(),
        )
        .then(|| "Asks you to trust this project's toolbar first".to_owned());
        let ui_builder = appearance.ui_builder().clone();
        let id = button.id.clone();
        toolbar_button(
            self.button_mouse_states
                .get(&button.id)
                .cloned()
                .unwrap_or_default(),
            label,
            padding,
            appearance,
        )
        .with_tooltip(move || match sublabel {
            Some(sublabel) => ui_builder
                .tool_tip_with_sublabel(tooltip, sublabel)
                .build()
                .finish(),
            None => ui_builder.tool_tip(tooltip).build().finish(),
        })
        .build()
        .on_click({
            let id = id.clone();
            move |ctx, _, _| {
                ctx.dispatch_typed_action(ProjectToolbarAction::ClickButton(id.clone()))
            }
        })
        .on_right_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(ProjectToolbarAction::OpenButtonMenu(id.clone()))
        })
        .finish()
    }

    fn render_overflow_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let color = theme.sub_text_color(theme.background());
        let ui_builder = appearance.ui_builder().clone();
        let mut button = toolbar_button(
            self.mouse_states.overflow_button.clone(),
            sized_icon(Icon::DotsHorizontal.to_warpui_icon(color).finish()),
            (4., 4.),
            appearance,
        )
        .with_tooltip(move || {
            ui_builder
                .tool_tip("More toolbar buttons".to_owned())
                .build()
                .finish()
        });
        if self.open_menu == Some(OpenMenu::Overflow) {
            button = button.active().with_active_styles(UiComponentStyles {
                background: Some(internal_colors::fg_overlay_3(theme).into()),
                ..UiComponentStyles::default()
            });
        }
        button
            .build()
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(ProjectToolbarAction::ToggleOverflowMenu)
            })
            .finish()
    }

    fn render_run_widget(&self, appearance: &Appearance, ctx: &AppContext) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text_color = theme.main_text_color(theme.background());
        let selected = self.selected_run_config(ctx);
        let chip_label = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                ConstrainedBox::new(
                    Text::new_inline(
                        selected.map(|item| item.label.clone()).unwrap_or_default(),
                        appearance.ui_font_family(),
                        FONT_SIZE,
                    )
                    .with_color(text_color.into_solid())
                    .with_style(Properties::default().weight(Weight::Medium))
                    .finish(),
                )
                .with_max_width(RUN_CHIP_MAX_LABEL_WIDTH)
                .finish(),
            )
            .with_child(
                Container::new(
                    ConstrainedBox::new(
                        Icon::ChevronDown
                            .to_warpui_icon(theme.sub_text_color(theme.background()))
                            .finish(),
                    )
                    .with_width(12.)
                    .with_height(12.)
                    .finish(),
                )
                .with_margin_left(4.)
                .finish(),
            )
            .finish();
        let ui_builder = appearance.ui_builder().clone();
        let mut chip = toolbar_button(
            self.mouse_states.run_config_chip.clone(),
            chip_label,
            (8., 6.),
            appearance,
        )
        .with_tooltip(move || {
            ui_builder
                .tool_tip("Run configurations".to_owned())
                .build()
                .finish()
        });
        if self.open_menu == Some(OpenMenu::RunConfigs) {
            chip = chip.active().with_active_styles(UiComponentStyles {
                background: Some(internal_colors::fg_overlay_3(theme).into()),
                ..UiComponentStyles::default()
            });
        }
        let chip = chip
            .build()
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(ProjectToolbarAction::ToggleRunConfigMenu)
            })
            .finish();

        let run_tooltip = selected
            .map(|item| format!("Run \"{}\"", item.label))
            .unwrap_or_default();
        let ui_builder = appearance.ui_builder().clone();
        let run = toolbar_button(
            self.mouse_states.run_button.clone(),
            sized_icon(WarpUiIcon::new(Icon::Play.into(), theme.ansi_fg_green()).finish()),
            (4., 4.),
            appearance,
        )
        .with_tooltip(move || ui_builder.tool_tip(run_tooltip).build().finish())
        .build()
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(ProjectToolbarAction::RunSelectedRunConfig))
        .finish();

        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        if self.shows_android(ctx) {
            row.add_child(
                Container::new(self.render_device_chip(appearance, ctx))
                    .with_margin_right(2.)
                    .finish(),
            );
        }
        row.with_child(chip)
            .with_child(Container::new(run).with_margin_left(2.).finish())
            .finish()
    }

    /// The device picker: the selected device (or the emulator being started) and a menu of
    /// devices and AVDs.
    fn render_device_chip(&self, appearance: &Appearance, ctx: &AppContext) -> Box<dyn Element> {
        let theme = appearance.theme();
        let android = AndroidModel::as_ref(ctx);
        let label = if self.pending_android_run.is_some() || android.is_any_launching() {
            "Starting emulator…".to_owned()
        } else {
            match android.selected_device() {
                Some(device) if device.entry.state.is_online() => device.display_name(),
                Some(device) => {
                    format!("{} ({})", device.display_name(), device.entry.state.label())
                }
                None => "No device".to_owned(),
            }
        };
        let sub_color = theme.sub_text_color(theme.background());
        let chip_label = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Container::new(sized_icon(Icon::Phone.to_warpui_icon(sub_color).finish()))
                    .with_margin_right(4.)
                    .finish(),
            )
            .with_child(
                ConstrainedBox::new(
                    Text::new_inline(label, appearance.ui_font_family(), FONT_SIZE)
                        .with_color(theme.main_text_color(theme.background()).into_solid())
                        .finish(),
                )
                .with_max_width(DEVICE_CHIP_MAX_LABEL_WIDTH)
                .finish(),
            )
            .with_child(
                Container::new(
                    ConstrainedBox::new(Icon::ChevronDown.to_warpui_icon(sub_color).finish())
                        .with_width(12.)
                        .with_height(12.)
                        .finish(),
                )
                .with_margin_left(4.)
                .finish(),
            )
            .finish();
        let ui_builder = appearance.ui_builder().clone();
        let mut chip = toolbar_button(
            self.mouse_states.device_chip.clone(),
            chip_label,
            (6., 6.),
            appearance,
        )
        .with_tooltip(move || {
            ui_builder
                .tool_tip("Android device".to_owned())
                .build()
                .finish()
        });
        if self.open_menu == Some(OpenMenu::Devices) {
            chip = chip.active().with_active_styles(UiComponentStyles {
                background: Some(internal_colors::fg_overlay_3(theme).into()),
                ..UiComponentStyles::default()
            });
        }
        chip.build()
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(ProjectToolbarAction::ToggleDeviceMenu))
            .finish()
    }

    fn render_trust_prompt(
        &self,
        prompt: &TrustPrompt,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let background = theme.surface_2();
        let main_text = theme.main_text_color(background).into_solid();
        let sub_text = theme.sub_text_color(background).into_solid();
        let root = self
            .project_root()
            .map(|root| root.display().to_string())
            .unwrap_or_default();

        let review = &prompt.review;
        let (title, description) = match &prompt.target {
            TrustTarget::TeamConfig { .. } if review.is_update => (
                "Trust this project's toolbar?",
                format!(
                    "{root}/.warpium/toolbar.toml changed since you trusted it ({}). Its commands \
                     run in your shell. Warpium asks again whenever the file changes.",
                    trust_change_summary(review)
                ),
            ),
            TrustTarget::TeamConfig { .. } => (
                "Trust this project's toolbar?",
                format!(
                    "These commands from {root}/.warpium/toolbar.toml run in your shell. \
                     Warpium asks again whenever the file changes."
                ),
            ),
            TrustTarget::AndroidBuild { .. } => (
                "Trust this project's Gradle build?",
                format!(
                    "Running the app runs the Gradle wrapper and build scripts of {root} in your \
                     shell. Warpium asks once per project."
                ),
            ),
        };
        let mut column = Flex::column()
            .with_child(
                Text::new(title, appearance.ui_font_family(), 14.)
                    .with_color(main_text)
                    .with_style(Properties::default().weight(Weight::Semibold))
                    .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(description, appearance.ui_font_family(), FONT_SIZE)
                        .with_color(sub_text)
                        .finish(),
                )
                .with_margin_top(6.)
                .with_margin_bottom(8.)
                .finish(),
            );

        let has_changes = review
            .listings
            .iter()
            .any(|listing| listing.change != TrustChange::Unchanged);
        let mut list = Flex::column();
        let mut shown_unchanged_heading = false;
        for listing in &review.listings {
            if review.is_update
                && has_changes
                && listing.change == TrustChange::Unchanged
                && !shown_unchanged_heading
            {
                shown_unchanged_heading = true;
                list.add_child(
                    Container::new(
                        Text::new_inline(
                            "Unchanged since you trusted it",
                            appearance.ui_font_family(),
                            FONT_SIZE,
                        )
                        .with_color(sub_text)
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .finish(),
                    )
                    .with_margin_top(4.)
                    .with_margin_bottom(6.)
                    .finish(),
                );
            }
            list.add_child(render_trust_listing(listing, review.is_update, appearance));
        }
        column.add_child(
            ConstrainedBox::new(
                ClippedScrollable::vertical(
                    prompt.list_scroll.clone(),
                    list.finish(),
                    ScrollbarWidth::Auto,
                    theme.nonactive_ui_detail().into(),
                    theme.active_ui_detail().into(),
                    Fill::None,
                )
                .with_overlayed_scrollbar()
                .finish(),
            )
            .with_max_height(TRUST_LIST_MAX_HEIGHT)
            .finish(),
        );
        let button_style = UiComponentStyles {
            font_size: Some(FONT_SIZE),
            font_weight: Some(Weight::Semibold),
            height: Some(28.),
            padding: Some(Coords {
                top: 0.,
                bottom: 0.,
                left: 12.,
                right: 12.,
            }),
            ..Default::default()
        };
        let buttons = Flex::row()
            .with_child(
                appearance
                    .ui_builder()
                    .button(
                        ButtonVariant::Accent,
                        self.mouse_states.trust_confirm.clone(),
                    )
                    .with_centered_text_label("Trust and run".to_owned())
                    .with_style(button_style)
                    .build()
                    .on_click(|ctx, _, _| {
                        ctx.dispatch_typed_action(ProjectToolbarAction::ConfirmTrust)
                    })
                    .finish(),
            )
            .with_child(
                Container::new(
                    appearance
                        .ui_builder()
                        .button(ButtonVariant::Basic, self.mouse_states.trust_cancel.clone())
                        .with_centered_text_label("Cancel".to_owned())
                        .with_style(button_style)
                        .build()
                        .on_click(|ctx, _, _| {
                            ctx.dispatch_typed_action(ProjectToolbarAction::CancelTrust)
                        })
                        .finish(),
                )
                .with_margin_left(8.)
                .finish(),
            )
            .finish();
        column.add_child(Container::new(buttons).with_margin_top(8.).finish());

        ConstrainedBox::new(
            Container::new(column.finish())
                .with_uniform_padding(16.)
                .with_background(background)
                .with_border(Border::all(1.).with_border_fill(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
                .with_drop_shadow(DropShadow::default())
                .finish(),
        )
        .with_width(TRUST_PROMPT_WIDTH)
        .finish()
    }
}

/// One command in the trust prompt, marked "New" or "Changed" when `mark_changes`.
fn render_trust_listing(
    listing: &TrustListing,
    mark_changes: bool,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let background = theme.surface_2();
    let main_text = theme.main_text_color(background).into_solid();
    let sub_text = theme.sub_text_color(background).into_solid();
    let mut heading = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
    let badge = match listing.change {
        TrustChange::New => Some(("New", theme.accent().into_solid())),
        TrustChange::Changed => Some(("Changed", theme.ui_warning_color())),
        TrustChange::Unchanged => None,
    };
    if let Some((badge, color)) = badge.filter(|_| mark_changes) {
        heading.add_child(
            Container::new(
                Text::new_inline(badge, appearance.ui_font_family(), FONT_SIZE)
                    .with_color(color)
                    .with_style(Properties::default().weight(Weight::Semibold))
                    .finish(),
            )
            .with_margin_right(6.)
            .finish(),
        );
    }
    heading.add_child(
        Shrinkable::new(
            1.,
            Text::new(
                format!("{} · {}", listing.label, listing.details),
                appearance.ui_font_family(),
                FONT_SIZE,
            )
            .with_color(sub_text)
            .finish(),
        )
        .finish(),
    );
    Container::new(
        Flex::column()
            .with_child(heading.finish())
            .with_child(
                Text::new(
                    listing.command.clone(),
                    appearance.monospace_font_family(),
                    FONT_SIZE,
                )
                .with_color(main_text)
                .finish(),
            )
            .finish(),
    )
    .with_margin_bottom(6.)
    .finish()
}

/// Such as `2 new, 1 changed, 5 unchanged`.
fn trust_change_summary(review: &TrustReview) -> String {
    [
        (review.count(TrustChange::New), "new"),
        (review.count(TrustChange::Changed), "changed"),
        (review.count(TrustChange::Unchanged), "unchanged"),
        (review.removed, "removed"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, kind)| format!("{count} {kind}"))
    .collect::<Vec<_>>()
    .join(", ")
}

/// The text of a project's two toolbar config files; `None` for a file that doesn't exist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolbarTexts {
    pub(crate) team: Option<String>,
    pub(crate) personal: Option<String>,
}

/// Reads both toolbar config files of the project at `root`.
pub(crate) fn read_toolbar_texts(root: &Path) -> Result<ToolbarTexts, String> {
    let read = |path: &Path| match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("couldn't read {}: {err}", path.display())),
    };
    Ok(ToolbarTexts {
        team: read(&team_config_path(root))?,
        personal: personal_path(root)
            .map(|path| read(&path))
            .transpose()?
            .flatten(),
    })
}

/// Writes the files a [`SavePlan`] changes, after checking that neither file changed since
/// `opened` was read. Writing the team file from the GUI counts as the user's own edit, so its
/// trust is carried over to the new version first (see [`ToolbarUserState::record_gui_save`]).
pub(crate) fn save_toolbar_plan<V: View>(
    root: &Path,
    opened: &ToolbarTexts,
    plan: &SavePlan,
    ctx: &mut ViewContext<V>,
) -> Result<(), String> {
    if read_toolbar_texts(root)? != *opened {
        return Err(
            "the toolbar config changed on disk while you were editing it; close the editor and \
             open it again"
                .to_owned(),
        );
    }
    if let Some(team) = &plan.team {
        write_config_file(&team_config_path(root), team)?;
        ensure_store(ctx);
        let previous = opened.team.clone();
        ProjectToolbarStore::handle(ctx).update(ctx, |store, ctx| {
            store.record_gui_save(root, previous.as_deref(), team, ctx)
        });
    }
    if let Some(personal) = &plan.personal {
        let path = personal_path(root)
            .ok_or_else(|| "couldn't find the home directory for personal buttons".to_owned())?;
        write_config_file(&path, personal)?;
    }
    Ok(())
}

fn write_config_file(path: &Path, text: &str) -> Result<(), String> {
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, text))
        .map_err(|err| {
            log::warn!("[Project toolbar] Failed to write a toolbar config: {err:#}");
            format!("couldn't write {}: {err}", path.display())
        })
}

/// Applies a context-menu change to a button of the project at `root` and saves it.
fn apply_quick_edit<V: View>(
    root: &Path,
    id: &str,
    edit: QuickEdit,
    ctx: &mut ViewContext<V>,
) -> Result<(), String> {
    let texts = read_toolbar_texts(root)?;
    let mut model = EditorModel::load(texts.team.as_deref(), texts.personal.as_deref())
        .map_err(|err| err.to_string())?;
    let index = model
        .index_of(id)
        .ok_or_else(|| "the button is no longer in the toolbar config".to_owned())?;
    match edit {
        QuickEdit::Duplicate => {
            model.duplicate(index);
        }
        QuickEdit::MoveLeft | QuickEdit::MoveRight => {
            let delta = if edit == QuickEdit::MoveLeft { -1 } else { 1 };
            if !move_past_visible_neighbor(&mut model, index, delta) {
                return Ok(());
            }
        }
        QuickEdit::Hide => model.buttons[index].hidden = true,
        // For a personal override, deleting drops the user's changes, not the shared button.
        QuickEdit::Delete if model.is_override(index) => model.revert_override(index),
        QuickEdit::Delete => model.remove(index),
    }
    let plan = model
        .plan_save(texts.team.as_deref(), texts.personal.as_deref())
        .map_err(|err| err.message)?;
    save_toolbar_plan(root, &texts, &plan, ctx)
}

/// Moves a button past its nearest neighbor shown inline on the toolbar in direction `delta`
/// (skipping hidden, overflow-only and run-configuration buttons); returns whether it moved.
fn move_past_visible_neighbor(model: &mut EditorModel, index: usize, delta: isize) -> bool {
    let mut probe = index;
    let target = loop {
        match model.move_target(probe, delta) {
            Some(next) => {
                probe = next;
                let button = &model.buttons[next];
                if !button.hidden && button.spec.placement == Placement::Toolbar {
                    break next;
                }
            }
            None => return false,
        }
    };
    let mut position = index;
    while position != target {
        match model.move_button(position, delta) {
            Some(next) => position = next,
            None => return false,
        }
    }
    true
}

/// Reads and merges the team and personal configs of the project at `root`.
fn load(root: &Path) -> LoadedConfig {
    let team_path = team_config_path(root);
    let personal = personal_path(root);
    let invalid = |error: ConfigError| {
        let path = match error.source {
            ConfigSource::Team => team_path.clone(),
            ConfigSource::Personal => personal.clone().unwrap_or_else(|| team_path.clone()),
        };
        LoadedConfig::Invalid { error, path }
    };
    let team = match read_config(&team_path, ConfigSource::Team) {
        Ok(team) => team,
        Err(error) => return invalid(error),
    };
    let personal_config = match personal.as_deref() {
        Some(path) => match read_config(path, ConfigSource::Personal) {
            Ok(config) => config,
            Err(error) => return invalid(error),
        },
        None => None,
    };
    let team_hash = team.as_ref().map(|(_, hash)| hash.clone());
    let team_only = team.as_ref().map(|(config, _)| merge(Some(config), None));
    match merge(
        team.as_ref().map(|(config, _)| config),
        personal_config.as_ref().map(|(config, _)| config),
    ) {
        Ok(buttons) => {
            let team_buttons = match team_only {
                Some(Ok(team_buttons)) => team_buttons,
                // The team file alone always merges when it merges with overrides; if not, the
                // merged buttons are a superset to review.
                Some(Err(_)) => buttons.clone(),
                None => Vec::new(),
            };
            LoadedConfig::Buttons {
                buttons,
                team_hash,
                team_buttons,
            }
        }
        Err(error) => invalid(error),
    }
}

/// Reads one config file and its content hash; a missing file is no config.
fn read_config(
    path: &Path,
    source: ConfigSource,
) -> Result<Option<(warpium_project_toolbar::ParsedConfig, String)>, ConfigError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(ConfigError {
                source,
                message: format!("couldn't read the file: {err}"),
                location: None,
            });
        }
    };
    let hash = content_hash(&bytes);
    let text = String::from_utf8(bytes).map_err(|_| ConfigError {
        source,
        message: "the file is not valid UTF-8".to_owned(),
        location: None,
    })?;
    parse_config(&text, source).map(|config| Some((config, hash)))
}

/// Names of the launch configurations with a session starting inside `root`.
fn project_launch_configs(root: &Path, ctx: &AppContext) -> Vec<String> {
    WarpConfig::as_ref(ctx)
        .launch_configs()
        .iter()
        .filter(|config| {
            let mut cwds = Vec::new();
            for window in &config.windows {
                for tab in &window.tabs {
                    collect_pane_cwds(&tab.layout, &mut cwds);
                }
            }
            cwds.iter().any(|cwd| {
                let cwd = dunce::canonicalize(cwd).unwrap_or_else(|_| cwd.clone());
                cwd.starts_with(root)
            })
        })
        .map(|config| config.name.clone())
        .collect()
}

fn collect_pane_cwds(layout: &PaneTemplateType, cwds: &mut Vec<PathBuf>) {
    match layout {
        PaneTemplateType::PaneTemplate { cwd, .. } => cwds.push(cwd.clone()),
        PaneTemplateType::PaneBranchTemplate { panes, .. } => {
            for pane in panes {
                collect_pane_cwds(pane, cwds);
            }
        }
    }
}

fn edit_menu_items() -> Vec<MenuItem<ProjectToolbarAction>> {
    vec![
        MenuItem::Separator,
        MenuItem::Item(
            MenuItemFields::new("Customize toolbar…")
                .with_on_select_action(ProjectToolbarAction::Customize(None)),
        ),
        MenuItem::Item(
            MenuItemFields::new("Edit project toolbar…")
                .with_on_select_action(ProjectToolbarAction::EditConfig { personal: false }),
        ),
        MenuItem::Item(
            MenuItemFields::new("Edit personal toolbar overrides…")
                .with_on_select_action(ProjectToolbarAction::EditConfig { personal: true }),
        ),
    ]
}

fn sized_icon(icon: Box<dyn Element>) -> Box<dyn Element> {
    ConstrainedBox::new(icon)
        .with_width(ICON_SIZE)
        .with_height(ICON_SIZE)
        .finish()
}

/// A title-bar button styled like the other top-right icons (see the workspace's
/// `render_right_panel_button`).
fn toolbar_button(
    mouse_state: MouseStateHandle,
    label: Box<dyn Element>,
    (padding_left, padding_right): (f32, f32),
    appearance: &Appearance,
) -> Button {
    let theme = appearance.theme();
    let default_styles = UiComponentStyles {
        font_color: Some(theme.sub_text_color(theme.background()).into()),
        font_size: Some(FONT_SIZE),
        font_family_id: Some(appearance.ui_font_family()),
        height: Some(BUTTON_HEIGHT),
        border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.))),
        border_width: Some(0.),
        padding: Some(Coords {
            top: 0.,
            bottom: 0.,
            left: padding_left,
            right: padding_right,
        }),
        ..Default::default()
    };
    let hover_styles = UiComponentStyles {
        background: Some(theme.surface_2().into()),
        ..default_styles
    };
    let clicked_styles = UiComponentStyles {
        background: Some(theme.background().into()),
        ..default_styles
    };
    Button::new(
        mouse_state,
        default_styles,
        Some(hover_styles),
        Some(clicked_styles),
        None,
    )
    .with_custom_label(label)
}

impl Entity for ProjectToolbarView {
    type Event = ProjectToolbarEvent;
}

impl TypedActionView for ProjectToolbarView {
    type Action = ProjectToolbarAction;

    fn handle_action(&mut self, action: &ProjectToolbarAction, ctx: &mut ViewContext<Self>) {
        match action {
            ProjectToolbarAction::ClickButton(id) => self.click_button(id, ctx),
            ProjectToolbarAction::ToggleRunConfigMenu => {
                self.toggle_menu(OpenMenu::RunConfigs, ctx)
            }
            ProjectToolbarAction::ToggleOverflowMenu => self.toggle_menu(OpenMenu::Overflow, ctx),
            ProjectToolbarAction::SelectRunConfig(key) => {
                self.open_menu = None;
                if let Some(root) = self.project_root().map(Path::to_path_buf) {
                    ProjectToolbarStore::handle(ctx).update(ctx, |store, ctx| {
                        store.update_state(|state| state.select_run_config(&root, key), ctx);
                    });
                }
                ctx.notify();
            }
            ProjectToolbarAction::RunSelectedRunConfig => self.run_selected_run_config(ctx),
            ProjectToolbarAction::OpenInvalidConfig => {
                if let Some(LoadedConfig::Invalid { path, error }) = &self.loaded {
                    ctx.emit(ProjectToolbarEvent::OpenFile {
                        path: path.clone(),
                        location: error.location,
                    });
                }
            }
            ProjectToolbarAction::ConfirmTrust => self.confirm_trust(ctx),
            ProjectToolbarAction::CancelTrust => {
                self.trust_prompt = None;
                ctx.notify();
            }
            ProjectToolbarAction::EditConfig { personal } => {
                self.open_menu = None;
                ctx.emit(ProjectToolbarEvent::EditConfig {
                    personal: *personal,
                });
                ctx.notify();
            }
            ProjectToolbarAction::ToggleDeviceMenu => self.toggle_menu(OpenMenu::Devices, ctx),
            ProjectToolbarAction::SelectAndroidDevice(serial) => {
                self.open_menu = None;
                let serial = serial.clone();
                AndroidModel::handle(ctx).update(ctx, |android, ctx| android.select(serial, ctx));
                ctx.emit(ProjectToolbarEvent::MenuClosed);
                ctx.notify();
            }
            ProjectToolbarAction::LaunchAndroidAvd(avd) => {
                self.open_menu = None;
                let avd = avd.clone();
                AndroidModel::handle(ctx).update(ctx, |android, ctx| android.launch_avd(avd, ctx));
                ctx.emit(ProjectToolbarEvent::MenuClosed);
                ctx.notify();
            }
            ProjectToolbarAction::OpenLogcat => {
                self.open_menu = None;
                ctx.emit(ProjectToolbarEvent::OpenLogcat);
                ctx.notify();
            }
            ProjectToolbarAction::Customize(select) => self.customize(select.clone(), false, ctx),
            ProjectToolbarAction::AddButton => self.customize(None, true, ctx),
            ProjectToolbarAction::OpenButtonMenu(id) => {
                self.context_button = Some(id.clone());
                self.open_menu = None;
                self.toggle_menu(OpenMenu::Button, ctx);
            }
            ProjectToolbarAction::QuickEdit { id, edit } => self.quick_edit(id, *edit, ctx),
        }
    }
}

impl View for ProjectToolbarView {
    fn ui_name() -> &'static str {
        "ProjectToolbar"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let (Some(root), Some(loaded)) = (self.project_root(), &self.loaded) else {
            return Empty::new().finish();
        };
        let appearance = Appearance::as_ref(app);
        let row = match loaded {
            LoadedConfig::Invalid { error, .. } => self.render_invalid_config(error, appearance),
            LoadedConfig::Buttons { buttons, .. } => {
                let toolbar_buttons = inline_buttons(buttons);
                let has_overflow_only = buttons
                    .iter()
                    .any(|button| button.overflow_only && !button.run_config);
                if toolbar_buttons.is_empty() && !has_overflow_only && self.run_configs.is_empty() {
                    self.render_add_button(true, appearance)
                } else {
                    self.render_button_row(
                        &toolbar_buttons,
                        has_overflow_only,
                        root,
                        appearance,
                        app,
                    )
                }
            }
        };
        self.render_with_overlays(row, appearance)
    }
}

impl ProjectToolbarView {
    /// The inline buttons (moving into the overflow menu as space runs out), the overflow menu
    /// button, the "+" button and the run-configurations widget.
    fn render_button_row(
        &self,
        toolbar_buttons: &[&ToolbarButton],
        has_overflow_only: bool,
        root: &Path,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let rendered_buttons = toolbar_buttons
            .iter()
            .map(|button| {
                Container::new(self.render_button(button, root, appearance, app))
                    .with_margin_left(2.)
                    .finish()
            })
            .collect::<Vec<_>>();
        let overflow_button = Container::new(self.render_overflow_button(appearance))
            .with_margin_left(2.)
            .finish();
        let mut trailing = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Container::new(self.render_add_button(false, appearance))
                    .with_margin_left(2.)
                    .finish(),
            );
        if !self.run_configs.is_empty() {
            let theme = appearance.theme();
            trailing.add_child(
                Container::new(
                    ConstrainedBox::new(Rect::new().with_background(theme.outline()).finish())
                        .with_width(1.)
                        .with_height(16.)
                        .finish(),
                )
                .with_margin_left(6.)
                .with_margin_right(4.)
                .finish(),
            );
            trailing.add_child(self.render_run_widget(appearance, app));
        }
        ToolbarRow::new(
            rendered_buttons,
            overflow_button,
            Some(trailing.finish()),
            has_overflow_only,
            self.visible_buttons.clone(),
        )
        .finish()
    }

    fn render_with_overlays(
        &self,
        row: Box<dyn Element>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let mut stack = Stack::new();
        stack.add_child(row);
        let positioning = OffsetPositioning::offset_from_parent(
            vec2f(0., 6.),
            ParentOffsetBounds::WindowByPosition,
            ParentAnchor::BottomRight,
            ChildAnchor::TopRight,
        );
        if let Some(prompt) = &self.trust_prompt {
            stack.add_positioned_overlay_child(
                self.render_trust_prompt(prompt, appearance),
                positioning,
            );
        } else if self.open_menu.is_some() {
            stack.add_positioned_overlay_child(ChildView::new(&self.menu).finish(), positioning);
        }
        Container::new(stack.finish()).with_margin_left(4.).finish()
    }
}
