//! Warpium: opening the Git Log pane and toggling the git settings.

use std::path::{Path, PathBuf};

use settings::ToggleableSetting as _;
use warp_errors::report_if_error;
use warpui::{SingletonEntity as _, ViewContext, ViewHandle};

use super::Workspace;
use crate::pane_group::{Direction, GitLogPane};
use crate::view_components::DismissibleToast;
use crate::warpium::git_log::{GitLogPaneManager, GitLogView};
use crate::warpium::git_tools::{self, WarpiumGitAction};
use crate::warpium::sync_settings::WarpiumGitSettings;
use crate::workspace::{ActiveSession, ToastStack};

impl Workspace {
    pub(super) fn handle_warpium_git_action(
        &mut self,
        action: &WarpiumGitAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WarpiumGitAction::ShowLog => {
                let dir = self.git_start_dir(ctx);
                self.open_git_log(dir, None, None, ctx);
            }
            WarpiumGitAction::ShowFileHistory(path) => {
                let dir = path.parent().map(Path::to_path_buf);
                self.open_git_log(dir, Some(path.clone()), None, ctx);
            }
            WarpiumGitAction::ShowCommit { path, oid } => {
                let dir = if path.is_dir() {
                    Some(path.clone())
                } else {
                    path.parent().map(Path::to_path_buf)
                };
                self.open_git_log(dir, None, Some(oid.clone()), ctx);
            }
            WarpiumGitAction::ToggleGutterMarkers => {
                WarpiumGitSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.git_gutter_markers.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            WarpiumGitAction::ToggleInlineBlame => {
                WarpiumGitSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.inline_blame.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
        }
    }

    /// Where to look for the active tab's repository: its locked project root, else the project
    /// toolbar's project (from the focused editor's file or the session), else the active
    /// session's directory.
    fn git_start_dir(&self, ctx: &ViewContext<Self>) -> Option<PathBuf> {
        let window_id = ctx.window_id();
        self.active_tab_pane_group()
            .as_ref(ctx)
            .project_root()
            .primary()
            .map(Path::to_path_buf)
            .or_else(|| {
                self.project_toolbar
                    .as_ref(ctx)
                    .detected_project_root()
                    .map(Path::to_path_buf)
            })
            .or_else(|| {
                ActiveSession::as_ref(ctx)
                    .path_if_local(window_id)
                    .map(Path::to_path_buf)
            })
    }

    /// Opens the window's Git Log pane below the active pane (or focuses the existing one) on the
    /// repository holding `dir`, listing `file`'s history when given and selecting `select`.
    fn open_git_log(
        &mut self,
        dir: Option<PathBuf>,
        file: Option<PathBuf>,
        select: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        if !git_tools::is_enabled() {
            return;
        }
        let Some(dir) = dir else {
            let window_id = ctx.window_id();
            ToastStack::handle(ctx).update(ctx, |stack, ctx| {
                stack.add_ephemeral_toast(
                    DismissibleToast::error(
                        "Open a folder or a terminal inside a git repository to see its log."
                            .to_owned(),
                    ),
                    window_id,
                    ctx,
                );
            });
            return;
        };
        let Some(view) = self.find_or_open_git_log_pane(ctx) else {
            return;
        };
        view.update(ctx, |view, ctx| {
            view.open(dir, file, select, ctx);
            view.focus(ctx);
        });
    }

    fn find_or_open_git_log_pane(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) -> Option<ViewHandle<GitLogView>> {
        if let Some(locator) = GitLogPaneManager::as_ref(ctx).find_pane(ctx.window_id()) {
            let view = self
                .tabs
                .iter()
                .find(|tab| tab.pane_group.id() == locator.pane_group_id)
                .map(|tab| tab.pane_group.clone())
                .and_then(|pane_group| {
                    pane_group.read(ctx, |pane_group, ctx| {
                        pane_group
                            .downcast_pane_by_id::<GitLogPane>(locator.pane_id)
                            .map(|pane| pane.git_log_view(ctx))
                    })
                });
            if view.is_some() {
                self.focus_pane(locator, ctx);
                return view;
            }
        }
        let pane = GitLogPane::new(ctx);
        let view = pane.git_log_view(ctx);
        self.active_tab_pane_group().update(ctx, |pane_group, ctx| {
            pane_group.add_pane_with_direction(Direction::Down, pane, true, ctx);
        });
        Some(view)
    }
}
