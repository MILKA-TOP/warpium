//! Warpium: opens the Find in Files popup for the active tab's project and opens its matches.

use std::path::{Path, PathBuf};

use warp_core::features::FeatureFlag;
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warp_util::path::{LineAndColumnArg, group_roots_by_common_ancestor};
use warpui::elements::{ChildView, Element, ParentElement, Stack};
use warpui::{AppContext, SingletonEntity, ViewContext, ViewHandle};

use super::Workspace;
use crate::code::editor_management::CodeSource;
use crate::settings::CodeSettings;
use crate::util::openable_file_type::{EditorLayout, FileTarget};
use crate::view_components::DismissibleToast;
use crate::warpium::find_in_files::{FindInFilesEvent, FindInFilesView, is_searchable_root};

impl Workspace {
    pub(super) fn build_find_in_files_overlay(
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<FindInFilesView> {
        let view = ctx.add_typed_action_view(FindInFilesView::new);
        ctx.subscribe_to_view(&view, |me, _, event, ctx| {
            me.handle_find_in_files_event(event, ctx);
        });
        view
    }

    /// Adds the Find in Files popup, when open, over the window.
    pub(super) fn add_find_in_files_overlay(&self, stack: &mut Stack, app: &AppContext) {
        if self.find_in_files_overlay.as_ref(app).is_open() {
            stack.add_child(ChildView::new(&self.find_in_files_overlay).finish());
        }
    }

    /// Opens the Find in Files popup scoped to the active tab's project (cmd-shift-F).
    pub(super) fn open_find_in_files(&mut self, ctx: &mut ViewContext<Self>) {
        if !FeatureFlag::GlobalSearch.is_enabled() || !*CodeSettings::as_ref(ctx).show_global_search
        {
            return;
        }
        let Some((roots, scope_label)) = self.find_in_files_scope(ctx) else {
            self.toast_stack.update(ctx, |toast_stack, ctx| {
                toast_stack.add_ephemeral_toast(
                    DismissibleToast::error(
                        "Find in Files: no local project folder to search".to_owned(),
                    ),
                    ctx,
                )
            });
            return;
        };
        let selected_text = self.get_selected_text_from_focused_view(ctx);
        let roots = roots.into_iter().map(LocalOrRemotePath::Local).collect();
        self.find_in_files_overlay.update(ctx, |view, ctx| {
            view.open(roots, scope_label, selected_text, ctx);
        });
        ctx.notify();
    }

    /// The local folders the popup searches: the tab's locked project roots, else the working
    /// directories Global Search uses. Never `$HOME` or `/`; remote hosts are skipped for now.
    pub(super) fn find_in_files_scope(&self, ctx: &AppContext) -> Option<(Vec<PathBuf>, String)> {
        let pane_group_handle = self.get_pane_group_view(self.active_tab_index)?;
        let pane_group = pane_group_handle.as_ref(ctx);
        let project_root = pane_group.project_root();
        let roots: Vec<LocalOrRemotePath> = if project_root.is_locked() {
            project_root
                .roots()
                .iter()
                .map(|root| LocalOrRemotePath::Local(root.clone()))
                .collect()
        } else {
            self.working_directories_model
                .as_ref(ctx)
                .most_recent_directories_for_pane_group(pane_group_handle.id())?
                .map(|directory| directory.path)
                .collect()
        };
        let home = dirs::home_dir();
        let local_roots: Vec<PathBuf> = roots
            .iter()
            .filter_map(|root| root.to_local_path().map(Path::to_path_buf))
            .filter(|path| is_searchable_root(path, home.as_deref()))
            .collect();
        // Ancestor-dedup so nested working directories aren't searched twice.
        let roots = group_roots_by_common_ancestor(&local_roots).roots;
        let primary = roots.first()?;
        let name = primary
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| primary.display().to_string());
        let scope_label = if roots.len() > 1 {
            format!("{name} (+{} more)", roots.len() - 1)
        } else {
            name
        };
        Some((roots, scope_label))
    }

    fn handle_find_in_files_event(
        &mut self,
        event: &FindInFilesEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            FindInFilesEvent::Close => {
                self.focus_active_tab(ctx);
                ctx.notify();
            }
            FindInFilesEvent::OpenMatch {
                path,
                line_number,
                column_num,
            } => {
                let line_col = LineAndColumnArg {
                    line_num: *line_number as usize,
                    column_num: *column_num,
                };
                let location = LocalOrRemotePath::Local(path.clone());
                self.open_file_with_target(
                    path.clone(),
                    FileTarget::CodeEditor(EditorLayout::SplitPane),
                    Some(line_col),
                    CodeSource::CommandPalette { location },
                    ctx,
                );
            }
        }
    }
}
