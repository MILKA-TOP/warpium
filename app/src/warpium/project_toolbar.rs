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

use notify_debouncer_full::notify::{RecursiveMode, WatchFilter};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::color::internal_colors;
use warp_core::user_preferences::GetUserPreferences as _;
use warpium_project_toolbar::{
    ButtonIcon, ConfigError, ConfigSource, ProjectContext, RunConfigItem, RunConfigKind,
    ToolbarButton, ToolbarUserState, TrustListing, content_hash, detect_project, merge,
    needs_trust, parse_config, personal_config_path, run_config_items, selected_run_config,
    team_config_path, toolbar_buttons, trust_listings,
};
use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{
    Border, CacheOption, ChildAnchor, ChildView, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, Element, Empty, Flex, Icon as WarpUiIcon, Image, MouseStateHandle,
    OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds, Radius, Rect, Stack, Text,
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
use crate::workspace::WorkspaceAction;

/// Private-preferences key holding the serialized [`ToolbarUserState`].
const STATE_KEY: &str = "WarpiumProjectToolbarState";
const WATCHER_DEBOUNCE: Duration = Duration::from_millis(300);
const BUTTON_HEIGHT: f32 = 24.;
const ICON_SIZE: f32 = 16.;
const FONT_SIZE: f32 = 12.;
const RUN_CHIP_MAX_LABEL_WIDTH: f32 = 160.;
const MENU_WIDTH: f32 = 240.;
const TRUST_PROMPT_WIDTH: f32 = 420.;
/// Commands listed in the trust prompt before the rest are summarized.
const MAX_TRUST_LISTINGS: usize = 8;

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
    EditConfig { personal: bool },
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
    OpenFile(PathBuf),
    /// The toolbar's menu closed, so focus can go back to the session.
    MenuClosed,
    /// [`ProjectToolbarView::has_content`] changed, so its title-bar slot appears or goes away.
    ContentChanged,
}

enum LoadedConfig {
    Buttons {
        buttons: Vec<ToolbarButton>,
        /// The [`content_hash`] of the team file, when there is one.
        team_hash: Option<String>,
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
}

struct TrustPrompt {
    /// The button to run once the user trusts the toolbar.
    pending_button_id: String,
    team_hash: String,
    listings: Vec<TrustListing>,
}

#[derive(Default)]
struct MouseStates {
    run_config_chip: MouseStateHandle,
    run_button: MouseStateHandle,
    overflow_button: MouseStateHandle,
    invalid_config_button: MouseStateHandle,
    trust_confirm: MouseStateHandle,
    trust_cancel: MouseStateHandle,
}

/// The toolbar of one window, following its active tab.
pub(crate) struct ProjectToolbarView {
    /// The last pinned explorer root and working directory, so the toolbar can be recomputed
    /// when it is turned back on.
    inputs: (Option<PathBuf>, Option<PathBuf>),
    project: Option<ProjectContext>,
    loaded: Option<LoadedConfig>,
    run_configs: Vec<RunConfigItem>,
    watched_dirs: Vec<PathBuf>,
    menu: ViewHandle<Menu<ProjectToolbarAction>>,
    open_menu: Option<OpenMenu>,
    trust_prompt: Option<TrustPrompt>,
    /// Per button id, kept across reloads.
    button_mouse_states: HashMap<String, MouseStateHandle>,
    mouse_states: MouseStates,
    /// How many toolbar buttons fit inline at the last layout; the overflow menu lists the rest.
    visible_buttons: Arc<AtomicUsize>,
    /// [`Self::has_content`] when it was last reported to the workspace.
    had_content: bool,
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
        ctx.subscribe_to_model(&WarpiumSettings::handle(ctx), |me, _, event, ctx| {
            if let WarpiumSettingsChangedEvent::ProjectToolbar { .. } = event {
                let (pinned_root, cwd) = me.inputs.clone();
                me.update_context(pinned_root, cwd, true, ctx);
            }
        });
        ctx.subscribe_to_model(&WarpConfig::handle(ctx), |me, _, event, ctx| {
            if let WarpConfigUpdateEvent::LaunchConfigs = event {
                me.refresh_run_configs(ctx);
            }
        });
        Self {
            inputs: (None, None),
            project: None,
            loaded: None,
            run_configs: Vec::new(),
            watched_dirs: Vec::new(),
            menu,
            open_menu: None,
            trust_prompt: None,
            button_mouse_states: HashMap::new(),
            mouse_states: MouseStates::default(),
            visible_buttons: Arc::new(AtomicUsize::new(usize::MAX)),
            had_content: false,
        }
    }

    /// Whether the toolbar shows anything: buttons, the run-configurations widget or an invalid
    /// config warning.
    pub(crate) fn has_content(&self) -> bool {
        match &self.loaded {
            Some(LoadedConfig::Invalid { .. }) => true,
            Some(LoadedConfig::Buttons { buttons, .. }) => {
                !toolbar_buttons(buttons).is_empty() || !self.run_configs.is_empty()
            }
            None => false,
        }
    }

    /// The detected project, if the toolbar is on.
    pub(crate) fn project_root(&self) -> Option<&Path> {
        self.project.as_ref().map(|project| project.root.as_path())
    }

    /// Follows the active tab: its pinned Project Explorer root and its session's working
    /// directory (local sessions only).
    pub(crate) fn set_context(
        &mut self,
        pinned_root: Option<PathBuf>,
        cwd: Option<PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.update_context(pinned_root, cwd, false, ctx);
    }

    fn update_context(
        &mut self,
        pinned_root: Option<PathBuf>,
        cwd: Option<PathBuf>,
        force: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if !force && self.inputs == (pinned_root.clone(), cwd.clone()) {
            return;
        }
        self.inputs = (pinned_root.clone(), cwd.clone());
        let project = if is_enabled(ctx) {
            let canonical = |path: PathBuf| dunce::canonicalize(&path).unwrap_or(path);
            detect_project(
                pinned_root.map(canonical).as_deref(),
                cwd.map(canonical).as_deref(),
            )
        } else {
            None
        };
        if project == self.project {
            return;
        }
        self.project = project;
        self.open_menu = None;
        self.trust_prompt = None;
        self.reload(ctx);
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
        if let Some(prompt) = &self.trust_prompt
            && self.button(&prompt.pending_button_id).is_none()
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
                    pending_button_id: id.to_owned(),
                    team_hash,
                    listings: trust_listings(self.buttons()),
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
        // The file may have changed while the prompt was open; the listed commands are only
        // trusted for the version they came from.
        if self.team_hash() != Some(prompt.team_hash.as_str()) {
            ctx.notify();
            return;
        }
        ProjectToolbarStore::handle(ctx).update(ctx, |store, ctx| {
            store.update_state(|state| state.trust(&root, &prompt.team_hash), ctx);
        });
        self.click_button(&prompt.pending_button_id, ctx);
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
        }
    }

    fn toggle_menu(&mut self, menu: OpenMenu, ctx: &mut ViewContext<Self>) {
        if self.open_menu == Some(menu) {
            self.open_menu = None;
            ctx.notify();
            return;
        }
        self.trust_prompt = None;
        if menu == OpenMenu::RunConfigs {
            self.refresh_run_configs(ctx);
        }
        let items = match menu {
            OpenMenu::RunConfigs => self.run_config_menu_items(),
            OpenMenu::Overflow => self.overflow_menu_items(),
        };
        let selected = (menu == OpenMenu::RunConfigs)
            .then(|| self.selected_run_config(ctx))
            .flatten()
            .map(|item| ProjectToolbarAction::SelectRunConfig(item.key.clone()));
        self.menu.update(ctx, |menu_view, ctx| {
            menu_view.set_items(items, ctx);
            if let Some(selected) = &selected {
                menu_view.set_selected_by_action(selected, ctx);
            }
        });
        self.open_menu = Some(menu);
        ctx.focus(&self.menu);
        ctx.notify();
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
        let mut items = toolbar_buttons(self.buttons())
            .into_iter()
            .skip(self.visible_buttons.load(Ordering::Relaxed))
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
        let detail = format!("{}\nClick to open the file.", error.message);
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
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(ProjectToolbarAction::ClickButton(id.clone()))
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

        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(chip)
            .with_child(Container::new(run).with_margin_left(2.).finish())
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

        let mut column = Flex::column()
            .with_child(
                Text::new(
                    "Trust this project's toolbar?",
                    appearance.ui_font_family(),
                    14.,
                )
                .with_color(main_text)
                .with_style(Properties::default().weight(Weight::Semibold))
                .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(
                        format!(
                            "These commands from {root}/.warpium/toolbar.toml run in your shell. \
                             Warpium asks again whenever the file changes."
                        ),
                        appearance.ui_font_family(),
                        FONT_SIZE,
                    )
                    .with_color(sub_text)
                    .finish(),
                )
                .with_margin_top(6.)
                .with_margin_bottom(8.)
                .finish(),
            );
        for listing in prompt.listings.iter().take(MAX_TRUST_LISTINGS) {
            column.add_child(
                Container::new(
                    Flex::column()
                        .with_child(
                            Text::new(
                                format!("{} · {}", listing.label, listing.details),
                                appearance.ui_font_family(),
                                FONT_SIZE,
                            )
                            .with_color(sub_text)
                            .finish(),
                        )
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
                .finish(),
            );
        }
        if prompt.listings.len() > MAX_TRUST_LISTINGS {
            column.add_child(
                Text::new(
                    format!(
                        "…and {} more. Open the file to review them all.",
                        prompt.listings.len() - MAX_TRUST_LISTINGS
                    ),
                    appearance.ui_font_family(),
                    FONT_SIZE,
                )
                .with_color(sub_text)
                .finish(),
            );
        }
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
    match merge(
        team.as_ref().map(|(config, _)| config),
        personal_config.as_ref().map(|(config, _)| config),
    ) {
        Ok(buttons) => LoadedConfig::Buttons { buttons, team_hash },
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
            });
        }
    };
    let hash = content_hash(&bytes);
    let text = String::from_utf8(bytes).map_err(|_| ConfigError {
        source,
        message: "the file is not valid UTF-8".to_owned(),
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
                if let Some(LoadedConfig::Invalid { path, .. }) = &self.loaded {
                    ctx.emit(ProjectToolbarEvent::OpenFile(path.clone()));
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
                let toolbar_buttons = toolbar_buttons(buttons);
                if toolbar_buttons.is_empty() && self.run_configs.is_empty() {
                    return Empty::new().finish();
                }
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
                let run_widget = (!self.run_configs.is_empty()).then(|| {
                    let mut run_widget =
                        Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
                    if !toolbar_buttons.is_empty() {
                        let theme = appearance.theme();
                        run_widget.add_child(
                            Container::new(
                                ConstrainedBox::new(
                                    Rect::new().with_background(theme.outline()).finish(),
                                )
                                .with_width(1.)
                                .with_height(16.)
                                .finish(),
                            )
                            .with_margin_left(6.)
                            .with_margin_right(4.)
                            .finish(),
                        );
                    }
                    run_widget.add_child(self.render_run_widget(appearance, app));
                    run_widget.finish()
                });
                ToolbarRow::new(
                    rendered_buttons,
                    overflow_button,
                    run_widget,
                    self.visible_buttons.clone(),
                )
                .finish()
            }
        };

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
