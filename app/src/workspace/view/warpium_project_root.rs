//! Warpium: changing a tab's project root (following its terminals, or locked to folders) and
//! telling whether the active terminal is inside a locked project.

use std::path::{Path, PathBuf};

use warpium_project_root::{
    ProjectRoot, TerminalCwd, TerminalPlacement, containing_root, lock_target,
};
use warpui::platform::FilePickerConfiguration;
use warpui::{AppContext, EntityId, SingletonEntity as _, ViewContext, ViewHandle};

use super::Workspace;
use crate::code::buffer_location::LocalOrRemotePath;
use crate::pane_group::PaneGroup;
use crate::util::repo_detection::{RepoDetectionSessionType, detect_possible_git_repo};
use crate::view_components::DismissibleToast;
use crate::warpium::project_root::ProjectRootAction;
use crate::workspace::ToastStack;

/// What to do with the folder the user picks.
#[derive(Clone, Copy)]
enum PickedFolder {
    /// Open it in a new tab locked to it.
    OpenInNewTab,
    /// Lock the tab with this pane group to it alone.
    LockTo(EntityId),
    /// Add it to the project of the tab with this pane group.
    AddTo(EntityId),
}

impl Workspace {
    pub(super) fn handle_project_root_action(
        &mut self,
        action: &ProjectRootAction,
        ctx: &mut ViewContext<Self>,
    ) {
        let pane_group = self.active_tab_pane_group().clone();
        match action {
            ProjectRootAction::OpenFolderAsProject => {
                self.pick_project_folder(PickedFolder::OpenInNewTab, ctx)
            }
            ProjectRootAction::ChangeFolder => {
                self.pick_project_folder(PickedFolder::LockTo(pane_group.id()), ctx)
            }
            ProjectRootAction::AddFolder => {
                self.pick_project_folder(PickedFolder::AddTo(pane_group.id()), ctx)
            }
            ProjectRootAction::LockToCurrentFolder => {
                self.lock_project_to_current_folder(&pane_group, ctx)
            }
            ProjectRootAction::Unlock => {
                self.set_project_root_for_pane_group(&pane_group, ProjectRoot::Follow, ctx)
            }
            ProjectRootAction::ToggleLock => {
                if pane_group.as_ref(ctx).project_root().is_locked() {
                    self.set_project_root_for_pane_group(&pane_group, ProjectRoot::Follow, ctx);
                } else {
                    self.lock_project_to_current_folder(&pane_group, ctx);
                }
            }
            ProjectRootAction::RemoveFolder(root) => {
                let project_root = pane_group
                    .as_ref(ctx)
                    .project_root()
                    .with_root_removed(root);
                self.set_project_root_for_pane_group(&pane_group, project_root, ctx);
            }
        }
    }

    /// Makes `project_root` the tab's project root, updates everything that uses it (see
    /// [`Self::apply_project_root_for_pane_group`]) and saves the session.
    pub(super) fn set_project_root_for_pane_group(
        &mut self,
        pane_group: &ViewHandle<PaneGroup>,
        project_root: ProjectRoot,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.apply_project_root_for_pane_group(pane_group, project_root, ctx) {
            ctx.dispatch_global_action("workspace:save_app", ());
        }
    }

    /// Makes `project_root` the tab's project root and updates everything that uses it: the
    /// explorer, global search and code review (through the working directories model) and the
    /// project toolbar. Doesn't save the session. Returns whether the project root changed.
    pub(super) fn apply_project_root_for_pane_group(
        &mut self,
        pane_group: &ViewHandle<PaneGroup>,
        project_root: ProjectRoot,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        // Terminal working directories are canonical; keep the roots comparable with them.
        let project_root = match project_root {
            ProjectRoot::Follow => ProjectRoot::Follow,
            ProjectRoot::Locked(roots) => {
                ProjectRoot::locked(roots.as_slice().iter().map(|root| canonical(root)))
            }
        };
        if pane_group.as_ref(ctx).project_root() == &project_root {
            return false;
        }
        log::info!(
            "[Project root] Tab project {} ({} folders)",
            if project_root.is_locked() {
                "locked"
            } else {
                "follows terminals"
            },
            project_root.roots().len()
        );
        self.detect_project_repositories(pane_group.id(), &project_root, ctx);
        pane_group.update(ctx, |pane_group, ctx| {
            pane_group.set_project_root(project_root);
            ctx.notify();
        });
        self.refresh_working_directories_for_pane_group(pane_group, ctx);
        #[cfg(not(target_family = "wasm"))]
        self.refresh_project_toolbar(ctx);
        self.left_panel_view.update(ctx, |_, ctx| ctx.notify());
        ctx.notify();
        true
    }

    /// Where the active terminal of `pane_group` is relative to its locked project, or `None`
    /// while the project follows its terminals.
    pub(super) fn active_terminal_project_placement(
        &self,
        pane_group: &ViewHandle<PaneGroup>,
        ctx: &AppContext,
    ) -> Option<TerminalPlacement> {
        let pane_group = pane_group.as_ref(ctx);
        let project_root = pane_group.project_root();
        if !project_root.is_locked() {
            return None;
        }
        let Some(terminal_view) = pane_group.active_session_view(ctx) else {
            return project_root.place_terminal(&TerminalCwd::Unknown);
        };
        let terminal_view = terminal_view.as_ref(ctx);
        let cwd = match terminal_view.pwd_as_local_or_remote(ctx) {
            Some(LocalOrRemotePath::Local(path)) => TerminalCwd::Host(path),
            Some(LocalOrRemotePath::Remote(_)) => TerminalCwd::NotOnHost,
            None => match (
                terminal_view.active_session_is_local(ctx),
                terminal_view.pwd(),
            ) {
                // No session or no directory reported yet.
                (None, _) | (_, None) => TerminalCwd::Unknown,
                // An SSH session without a remote server.
                (Some(false), Some(_)) => TerminalCwd::NotOnHost,
                // A local session reporting a directory that doesn't exist here: a container's
                // path, unless it is lexically inside the project (a directory deleted under the
                // terminal).
                (Some(true), Some(pwd)) => {
                    let path = PathBuf::from(pwd);
                    if containing_root(project_root.roots(), &path).is_some() {
                        TerminalCwd::Host(path)
                    } else {
                        TerminalCwd::NotOnHost
                    }
                }
            },
        };
        project_root.place_terminal(&cwd)
    }

    fn lock_project_to_current_folder(
        &mut self,
        pane_group: &ViewHandle<PaneGroup>,
        ctx: &mut ViewContext<Self>,
    ) {
        match self.current_folder_lock_target(pane_group, ctx) {
            Some(root) => {
                self.set_project_root_for_pane_group(pane_group, ProjectRoot::locked_to(root), ctx)
            }
            None => self.show_project_root_toast(
                "There is no folder to lock the project to. Use Change project folder… to pick one.",
                ctx,
            ),
        }
    }

    /// The folder "Lock project to current folder" locks `pane_group`'s project to (see
    /// [`lock_target`]).
    fn current_folder_lock_target(
        &self,
        pane_group: &ViewHandle<PaneGroup>,
        ctx: &AppContext,
    ) -> Option<PathBuf> {
        let explorer_roots: Vec<PathBuf> = self
            .working_directories_model
            .as_ref(ctx)
            .most_recent_directories_for_pane_group(pane_group.id())
            .map(|directories| {
                directories
                    .filter_map(|directory| directory.path.to_local_path().map(Path::to_path_buf))
                    .collect()
            })
            .unwrap_or_default();
        let active_cwd = pane_group
            .as_ref(ctx)
            .active_session_view(ctx)
            .and_then(|view| view.as_ref(ctx).active_session_path_if_local(ctx))
            .map(|path| canonical(&path));
        lock_target(&explorer_roots, active_cwd.as_deref())
    }

    fn pick_project_folder(&mut self, then: PickedFolder, ctx: &mut ViewContext<Self>) {
        ctx.open_file_picker(
            move |result, ctx| {
                let path = match result {
                    Ok(paths) => match paths.into_iter().next() {
                        Some(path) => PathBuf::from(path),
                        None => return,
                    },
                    Err(err) => {
                        log::warn!("[Project root] Folder picker failed: {err:#}");
                        return;
                    }
                };
                let Some(workspace) = ctx.handle().upgrade(ctx) else {
                    return;
                };
                workspace.update(ctx, |workspace, ctx| {
                    workspace.apply_picked_folder(then, path, ctx);
                });
            },
            FilePickerConfiguration::new().folders_only(),
        );
    }

    fn apply_picked_folder(
        &mut self,
        then: PickedFolder,
        path: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) {
        let pane_group_id = match then {
            PickedFolder::OpenInNewTab => {
                self.open_directory_in_new_tab(path, ctx);
                return;
            }
            PickedFolder::LockTo(id) | PickedFolder::AddTo(id) => id,
        };
        // The tab may have been closed while the picker was open.
        let Some(pane_group) = self.get_pane_group_view_with_id(pane_group_id).cloned() else {
            return;
        };
        let path = canonical(&path);
        let project_root = match then {
            PickedFolder::AddTo(_) => {
                let current = pane_group.as_ref(ctx).project_root().clone();
                // A following tab keeps the folder it shows and gains the picked one.
                let current = match current {
                    ProjectRoot::Follow => self
                        .current_folder_lock_target(&pane_group, ctx)
                        .map_or(ProjectRoot::Follow, ProjectRoot::locked_to),
                    locked => locked,
                };
                let added = current.with_root_added(path);
                if added == current {
                    self.show_project_root_toast(
                        "That folder is already in the project, or inside or around one of its folders.",
                        ctx,
                    );
                }
                added
            }
            PickedFolder::LockTo(_) | PickedFolder::OpenInNewTab => ProjectRoot::locked_to(path),
        };
        self.set_project_root_for_pane_group(&pane_group, project_root, ctx);
    }

    /// Detects the git repositories of the locked folders, which no terminal may have visited,
    /// so code review can find them; the tab is refreshed once each is known.
    fn detect_project_repositories(
        &mut self,
        pane_group_id: EntityId,
        project_root: &ProjectRoot,
        ctx: &mut ViewContext<Self>,
    ) {
        for root in project_root.roots() {
            let Some(root) = root.to_str() else {
                continue;
            };
            let detection = detect_possible_git_repo(
                RepoDetectionSessionType::Local,
                root,
                repo_metadata::repositories::RepoDetectionSource::CodeReviewInitialization,
                ctx,
            );
            ctx.spawn(detection, move |workspace, repo, ctx| {
                if repo.is_none() {
                    return;
                }
                if let Some(pane_group) = workspace.get_pane_group_view_with_id(pane_group_id) {
                    let pane_group = pane_group.clone();
                    workspace.refresh_working_directories_for_pane_group(&pane_group, ctx);
                }
            });
        }
    }

    fn show_project_root_toast(&self, message: &str, ctx: &mut ViewContext<Self>) {
        let window_id = ctx.window_id();
        let toast = DismissibleToast::default(message.to_owned());
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(toast, window_id, ctx);
        });
    }
}

/// `path` with symlinks resolved, or as it is when it can't be (it may have been deleted).
fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
