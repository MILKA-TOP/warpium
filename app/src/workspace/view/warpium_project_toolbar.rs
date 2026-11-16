//! Warpium: runs the project toolbar's buttons and opens its config files.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use warp_util::path::LineAndColumnArg;
use warpium_project_toolbar::{
    BuiltinAction, ButtonAction, CommandAction, CommandTarget, PERSONAL_TEMPLATE, ShellFamily,
    TEAM_TEMPLATE, ToolbarButton, resolve_cwd, shell_line, team_config_path,
};
use warpui::elements::{Align, ChildView, Element, Shrinkable};
use warpui::{AppContext, SingletonEntity as _, TypedActionView as _, ViewContext, ViewHandle};

use super::Workspace;
use crate::cloud_object::model::persistence::CloudModel;
use crate::launch_configs::launch_config::LaunchConfig;
use crate::palette::PaletteMode;
use crate::pane_group::{Direction, FilePane, NewTerminalOptions, PanesLayout};
use crate::root_view::OpenLaunchConfigArg;
use crate::server::telemetry::{LaunchConfigUiLocation, PaletteSource};
use crate::settings_view::SettingsSection;
use crate::terminal::TerminalView;
use crate::terminal::shell::ShellType;
use crate::user_config::WarpConfig;
use crate::view_components::DismissibleToast;
use crate::warpium::project_toolbar::{
    ProjectLocation, ProjectToolbarEvent, ProjectToolbarView, personal_path,
};
use crate::workflows::{WorkflowSelectionSource, WorkflowSource, WorkflowType};
use crate::workspace::WorkspaceAction;
use crate::workspace::util::TerminalSessionFallbackBehavior;

/// The toolbar's flex factor in the title bar, relative to each tab's 1.
const PROJECT_TOOLBAR_FLEX: f32 = 1.;

impl Workspace {
    pub(super) fn build_project_toolbar_view(
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<ProjectToolbarView> {
        let toolbar = ctx.add_typed_action_view(ProjectToolbarView::new);
        ctx.subscribe_to_view(&toolbar, |me, _, event, ctx| {
            me.handle_project_toolbar_event(event, ctx);
        });
        toolbar
    }

    /// The project toolbar's place in the title bar: it takes a share of the width the tabs leave
    /// and keeps to the right of it, so its buttons collapse into the overflow menu as the title
    /// bar fills up.
    pub(super) fn render_project_toolbar_slot(&self, ctx: &AppContext) -> Option<Box<dyn Element>> {
        if !self.project_toolbar.as_ref(ctx).has_content() {
            return None;
        }
        let slot = Shrinkable::new(
            PROJECT_TOOLBAR_FLEX,
            Align::new(ChildView::new(&self.project_toolbar).finish())
                .right()
                .finish(),
        )
        .finish();
        Some(slot)
    }

    /// Points the toolbar at the active tab's project.
    pub(super) fn refresh_project_toolbar(&mut self, ctx: &mut ViewContext<Self>) {
        self.update_project_toolbar_context(false, ctx);
    }

    /// Like [`Self::refresh_project_toolbar`], but `force` detects the project again even when
    /// the tab's inputs are unchanged (a `.git` or `.warpium` may have been created since).
    fn update_project_toolbar_context(&mut self, force: bool, ctx: &mut ViewContext<Self>) {
        // Title and focus events can arrive while a tab is closing and the index is out of range.
        let Some((pinned_root, location)) = self.project_toolbar_inputs(ctx) else {
            return;
        };
        self.project_toolbar.update(ctx, |toolbar, ctx| {
            toolbar.set_context(pinned_root, location, force, ctx);
        });
    }

    /// The active tab's pinned Project Explorer root and where it is working: the folder of the
    /// focused editor's file, else the working directory of the tab's active (most recently
    /// focused) session if it is local.
    fn project_toolbar_inputs(
        &self,
        ctx: &AppContext,
    ) -> Option<(Option<PathBuf>, ProjectLocation)> {
        let pane_group = self.get_pane_group_view(self.active_tab_index)?.as_ref(ctx);
        let pinned_root = pane_group.pinned_explorer_root().cloned();
        let focused = pane_group.focused_pane_id(ctx);
        let editor_file = pane_group
            .code_pane_by_id(focused)
            .and_then(|pane| pane.file_view(ctx).as_ref(ctx).local_path(ctx))
            .or_else(|| {
                pane_group
                    .downcast_pane_by_id::<FilePane>(focused)
                    .and_then(|pane| pane.file_view(ctx).as_ref(ctx).local_path())
            });
        if let Some(dir) = editor_file
            .as_deref()
            .and_then(Path::parent)
            .filter(|dir| !dir.as_os_str().is_empty())
        {
            return Some((pinned_root, ProjectLocation::Directory(dir.to_path_buf())));
        }
        let location = match pane_group.active_session_view(ctx) {
            Some(view) => match view.as_ref(ctx).active_session_path_if_local(ctx) {
                Some(cwd) => ProjectLocation::Directory(cwd),
                None => ProjectLocation::NoProject,
            },
            None => ProjectLocation::Unknown,
        };
        Some((pinned_root, location))
    }

    fn handle_project_toolbar_event(
        &mut self,
        event: &ProjectToolbarEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            ProjectToolbarEvent::Run { root, button } => {
                self.run_project_toolbar_button(root, button, ctx)
            }
            ProjectToolbarEvent::OpenLaunchConfig(name) => {
                self.open_project_toolbar_launch_config(name, ctx)
            }
            ProjectToolbarEvent::EditConfig { personal } => {
                self.edit_project_toolbar_config(*personal, ctx)
            }
            ProjectToolbarEvent::OpenFile { path, location } => {
                // The editor's columns start at 0.
                let line_and_column = location.map(|location| LineAndColumnArg {
                    line_num: location.line,
                    column_num: Some(location.column.saturating_sub(1)),
                });
                self.add_tab_for_code_file(path.clone(), line_and_column, ctx)
            }
            ProjectToolbarEvent::MenuClosed => self.focus_active_tab(ctx),
            ProjectToolbarEvent::ContentChanged => ctx.notify(),
            ProjectToolbarEvent::RunAndroidApp { root, module } => {
                self.run_android_app(root, module, ctx)
            }
            ProjectToolbarEvent::OpenLogcat => {
                self.open_android_logcat(None, true, ctx);
            }
        }
    }

    fn run_project_toolbar_button(
        &mut self,
        root: &Path,
        button: &ToolbarButton,
        ctx: &mut ViewContext<Self>,
    ) {
        match &button.action {
            ButtonAction::Command(command) => {
                self.run_project_toolbar_command(root, command, button.display_label(), ctx)
            }
            ButtonAction::Workflow(name) => self.run_project_toolbar_workflow(root, name, ctx),
            ButtonAction::LaunchConfig(name) => self.open_project_toolbar_launch_config(name, ctx),
            ButtonAction::TabConfig(name) => {
                let tab_config = WarpConfig::as_ref(ctx)
                    .tab_configs()
                    .iter()
                    .find(|config| config.name == *name)
                    .cloned();
                match tab_config {
                    Some(tab_config) => self.open_tab_config(tab_config, ctx),
                    None => self.show_project_toolbar_error(
                        format!("Project toolbar: no tab config named \"{name}\""),
                        ctx,
                    ),
                }
            }
            ButtonAction::Url(url) => ctx.open_url(url),
            ButtonAction::Builtin(action) => {
                let action = workspace_action_for_builtin(*action);
                self.handle_action(&action, ctx);
            }
        }
    }

    /// Runs a command as an ordinary block, in the active session or a new tab or split.
    pub(super) fn run_project_toolbar_command(
        &mut self,
        root: &Path,
        command: &CommandAction,
        title: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let cwd = resolve_cwd(root, command.cwd.as_deref());
        if !cwd.is_dir() {
            self.show_project_toolbar_error(
                format!(
                    "Project toolbar: the folder {} doesn't exist",
                    cwd.display()
                ),
                ctx,
            );
            return;
        }
        let env = command
            .env
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect::<HashMap<_, _>>();
        let target = match command.target {
            CommandTarget::Current => {
                if let Some(session) = self.session_for_current_target(root, ctx) {
                    let (shell, session_cwd) = session.read(ctx, |view, ctx| {
                        (
                            view.active_session_shell_type(ctx),
                            view.active_session_path_if_local(ctx),
                        )
                    });
                    let shell = match shell {
                        Some(ShellType::Fish) => ShellFamily::Fish,
                        Some(ShellType::PowerShell) => ShellFamily::PowerShell,
                        Some(ShellType::Bash | ShellType::Zsh) | None => ShellFamily::Posix,
                    };
                    let already_there = session_cwd
                        .map(|session_cwd| dunce::canonicalize(&session_cwd).unwrap_or(session_cwd))
                        .is_some_and(|session_cwd| session_cwd == cwd);
                    let line = shell_line(
                        &command.command,
                        (!already_there).then_some(cwd.as_path()),
                        &command.env,
                        shell,
                    );
                    session.update(ctx, |view, ctx| {
                        view.execute_command_or_set_pending(&line, ctx)
                    });
                    return;
                }
                // No session can take the command here; run it in a new tab instead.
                CommandTarget::NewTab
            }
            target @ (CommandTarget::NewTab
            | CommandTarget::SplitRight
            | CommandTarget::SplitDown) => target,
        };
        let session = match target {
            CommandTarget::SplitRight | CommandTarget::SplitDown => {
                let direction = if target == CommandTarget::SplitRight {
                    Direction::Right
                } else {
                    Direction::Down
                };
                self.active_tab_pane_group().update(ctx, |pane_group, ctx| {
                    pane_group.add_terminal_pane_for_toolbar_command(direction, cwd, env, ctx)
                })
            }
            CommandTarget::Current | CommandTarget::NewTab => {
                let mut options = NewTerminalOptions::default().with_initial_directory(cwd);
                options.env_vars = env;
                self.add_tab_with_pane_layout(
                    PanesLayout::SingleTerminal(Box::new(options)),
                    Arc::new(HashMap::new()),
                    Some(title.to_owned()),
                    ctx,
                );
                let Some(session) = self
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .active_session_view(ctx)
                else {
                    return;
                };
                session
            }
        };
        session.update(ctx, |view, ctx| {
            view.execute_command_or_set_pending(&command.command, ctx)
        });
    }

    /// The session a `current` command runs in: the focused session, or when another kind of pane
    /// (such as an editor) is focused, the tab's most recently focused session if its working
    /// directory is inside the project at `root`. Either way the session has to be local, not
    /// running a command and have nothing typed in its input (which the command would replace).
    fn session_for_current_target(
        &self,
        root: &Path,
        ctx: &AppContext,
    ) -> Option<ViewHandle<TerminalView>> {
        let pane_group = self.active_tab_pane_group().as_ref(ctx);
        let session = match pane_group.focused_session_view(ctx) {
            Some(session) => session,
            None => pane_group.active_session_view(ctx).filter(|session| {
                session
                    .as_ref(ctx)
                    .active_session_path_if_local(ctx)
                    .map(|cwd| dunce::canonicalize(&cwd).unwrap_or(cwd))
                    .is_some_and(|cwd| cwd.starts_with(root))
            })?,
        };
        let view = session.as_ref(ctx);
        let is_executing = view.model.lock().block_list().active_block().is_executing();
        let is_idle = view.active_session_is_local(ctx).unwrap_or(false)
            && !is_executing
            && view.input().as_ref(ctx).buffer_text(ctx).trim().is_empty();
        is_idle.then_some(session)
    }

    /// Runs a saved workflow by name: the project's `.warp/workflows` first, then the user's
    /// local workflows, then Warp Drive.
    fn run_project_toolbar_workflow(
        &mut self,
        root: &Path,
        name: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        #[cfg(feature = "local_fs")]
        let project_workflow = crate::workflows::local_workflows::LocalWorkflows::handle(ctx)
            .update(ctx, |workflows, _| {
                workflows
                    .project_workflows(root, crate::workflows::local_workflows::UseCache::No)
                    .find(|workflow| workflow.name() == name)
                    .cloned()
            });
        #[cfg(not(feature = "local_fs"))]
        let project_workflow = {
            let _ = root;
            None
        };
        let local = project_workflow
            .map(|workflow| (workflow, WorkflowSource::Project))
            .or_else(|| {
                WarpConfig::as_ref(ctx)
                    .local_user_workflows()
                    .iter()
                    .find(|workflow| workflow.name() == name)
                    .cloned()
                    .map(|workflow| (workflow, WorkflowSource::Local))
            });
        if let Some((workflow, source)) = local {
            self.run_workflow_in_active_input(
                &WorkflowType::Local(workflow),
                source,
                WorkflowSelectionSource::Undefined,
                None,
                TerminalSessionFallbackBehavior::default(),
                ctx,
            );
            return;
        }
        let cloud_workflow = CloudModel::as_ref(ctx)
            .get_all_active_workflows()
            .find(|workflow| workflow.model().data.name() == name)
            .cloned();
        match cloud_workflow {
            Some(workflow) => self.run_cloud_workflow_in_active_input(
                workflow,
                WorkflowSelectionSource::Undefined,
                TerminalSessionFallbackBehavior::default(),
                ctx,
            ),
            None => self.show_project_toolbar_error(
                format!("Project toolbar: no workflow named \"{name}\""),
                ctx,
            ),
        }
    }

    fn open_project_toolbar_launch_config(&mut self, name: &str, ctx: &mut ViewContext<Self>) {
        let launch_config: Option<LaunchConfig> = WarpConfig::as_ref(ctx)
            .launch_configs()
            .iter()
            .find(|config| config.name == name)
            .cloned();
        match launch_config {
            Some(launch_config) => ctx.dispatch_global_action(
                "root_view:open_launch_config",
                OpenLaunchConfigArg {
                    launch_config,
                    ui_location: LaunchConfigUiLocation::AppMenu,
                    open_in_active_window: false,
                },
            ),
            None => self.show_project_toolbar_error(
                format!("Project toolbar: no launch configuration named \"{name}\""),
                ctx,
            ),
        }
    }

    /// Opens the project's team config (or the personal overrides), first creating it from the
    /// commented template. Only the user-authored `.warpium/toolbar.toml` is ever written into
    /// the project.
    pub(super) fn edit_project_toolbar_config(
        &mut self,
        personal: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        self.update_project_toolbar_context(true, ctx);
        let Some(root) = self
            .project_toolbar
            .as_ref(ctx)
            .detected_project_root()
            .map(Path::to_path_buf)
        else {
            self.show_project_toolbar_error(
                "No project found for this tab. Open a folder inside a git repository, or pin a \
                 Project Explorer root."
                    .to_owned(),
                ctx,
            );
            return;
        };
        let (path, template) = if personal {
            let Some(path) = personal_path(&root) else {
                self.show_project_toolbar_error(
                    "Couldn't find the home directory for personal toolbar overrides.".to_owned(),
                    ctx,
                );
                return;
            };
            (path, PERSONAL_TEMPLATE)
        } else {
            (team_config_path(&root), TEAM_TEMPLATE)
        };
        if !path.exists() {
            let created = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, template));
            if let Err(err) = created {
                log::warn!("[Project toolbar] Failed to create a toolbar config: {err:#}");
                self.show_project_toolbar_error(
                    format!("Couldn't create {}: {err}", path.display()),
                    ctx,
                );
                return;
            }
        }
        self.project_toolbar
            .update(ctx, |toolbar, ctx| toolbar.reload(ctx));
        self.add_tab_for_code_file(path, None, ctx);
    }

    pub(super) fn show_project_toolbar_error(
        &mut self,
        message: String,
        ctx: &mut ViewContext<Self>,
    ) {
        self.toast_stack.update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(DismissibleToast::error(message), ctx)
        });
    }
}

/// The workspace action a built-in toolbar action runs.
fn workspace_action_for_builtin(action: BuiltinAction) -> WorkspaceAction {
    match action {
        BuiltinAction::WindowCreate => WorkspaceAction::AddWindow,
        BuiltinAction::TabCreate => WorkspaceAction::AddDefaultTab,
        BuiltinAction::SessionNext => WorkspaceAction::CycleNextSession,
        BuiltinAction::SessionPrevious => WorkspaceAction::CyclePrevSession,
        BuiltinAction::SessionReopenClosed => WorkspaceAction::ReopenClosedSession,
        BuiltinAction::AppearanceFontSizeIncrease => WorkspaceAction::IncreaseFontSize,
        BuiltinAction::AppearanceFontSizeDecrease => WorkspaceAction::DecreaseFontSize,
        BuiltinAction::AppearanceFontSizeReset => WorkspaceAction::ResetFontSize,
        BuiltinAction::AppearanceZoomIncrease => WorkspaceAction::IncreaseZoom,
        BuiltinAction::AppearanceZoomDecrease => WorkspaceAction::DecreaseZoom,
        BuiltinAction::AppearanceZoomReset => WorkspaceAction::ResetZoom,
        BuiltinAction::SurfaceSettingsOpen => WorkspaceAction::ShowSettings,
        BuiltinAction::SurfaceCommandPaletteOpen => WorkspaceAction::OpenPalette {
            mode: PaletteMode::Command,
            source: PaletteSource::Keybinding,
            query: None,
        },
        BuiltinAction::SurfaceThemePickerOpen => WorkspaceAction::ShowThemeChooserForActiveTheme,
        BuiltinAction::SurfaceKeybindingsOpen => {
            WorkspaceAction::ShowSettingsPage(SettingsSection::Keybindings)
        }
        BuiltinAction::SurfaceWarpDriveOpen => WorkspaceAction::OpenWarpDrive,
        BuiltinAction::SurfaceWarpDriveToggle => WorkspaceAction::ToggleWarpDrive,
        BuiltinAction::SurfaceResourceCenterToggle => WorkspaceAction::ToggleResourceCenter,
        BuiltinAction::SurfaceCodeReviewToggle | BuiltinAction::SurfaceRightPanelToggle => {
            WorkspaceAction::ToggleRightPanel
        }
        BuiltinAction::SurfaceProjectExplorerOpen => WorkspaceAction::OpenProjectExplorer,
        BuiltinAction::SurfaceGlobalSearchOpen => WorkspaceAction::OpenGlobalSearch,
        BuiltinAction::SurfaceConversationListOpen => WorkspaceAction::OpenConversationListView,
        BuiltinAction::SurfaceLeftPanelToggle => WorkspaceAction::ToggleLeftPanel,
        BuiltinAction::SurfaceVerticalTabsToggle => WorkspaceAction::ToggleVerticalTabsPanel,
    }
}
