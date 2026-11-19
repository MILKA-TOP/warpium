//! Warpium: opening the palette as the cmd-O project search, with its All, Classes, Symbols and
//! Files tabs (`crate::warpium::search_everywhere`).

#[cfg(not(target_family = "wasm"))]
use std::path::{Path, PathBuf};

#[cfg(not(target_family = "wasm"))]
use warpui::SingletonEntity as _;
use warpui::ViewContext;

use super::Workspace;
#[cfg(not(target_family = "wasm"))]
use crate::warpium::code_index;
use crate::warpium::search_everywhere::SearchTab;
#[cfg(not(target_family = "wasm"))]
use crate::workspace::ActiveSession;

impl Workspace {
    /// Opens the project search on `tab`. On the All tab (cmd-O) only a Kotlin/Java project gets
    /// the search, and `false` means the caller should open the Files palette instead; the other
    /// tabs always open. When the palette is already open as the project search (`was_open`), it
    /// only switches to `tab`, keeping the query.
    pub(super) fn open_warpium_search_everywhere(
        &mut self,
        tab: SearchTab,
        was_open: bool,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if was_open && self.palette.as_ref(ctx).shows_warpium_search_tabs(ctx) {
            self.palette.update(ctx, |palette, ctx| {
                palette.show_warpium_search_tab(tab, ctx)
            });
            return true;
        }
        let is_shared_session_viewer = self.is_shared_session_viewer_focused(ctx);
        if !self.prepare_code_index_for_search(tab, is_shared_session_viewer, ctx) {
            return false;
        }
        self.palette.update(ctx, |palette, ctx| {
            palette.reset(ctx);
            let mixer = palette.search_bar.as_ref(ctx).mixer().clone();
            palette.data_source_store.update(ctx, |store, ctx| {
                store.reset_search_mixer(mixer, is_shared_session_viewer, ctx);
            });
            palette.show_warpium_search_tab(tab, ctx);
        });
        true
    }

    /// Indexes the active tab's project for the project search. Returns whether the search should
    /// open (see [`Workspace::open_warpium_search_everywhere`]).
    #[cfg(not(target_family = "wasm"))]
    fn prepare_code_index_for_search(
        &self,
        tab: SearchTab,
        is_shared_session_viewer: bool,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let (root, require_build_files) = self.code_index_project_root(ctx);
        match tab {
            SearchTab::All => {
                !is_shared_session_viewer
                    && code_index::open_project_for_search_everywhere(
                        root,
                        require_build_files,
                        ctx,
                    )
            }
            SearchTab::Classes | SearchTab::Symbols | SearchTab::Files => {
                code_index::ensure_project(root, require_build_files, ctx);
                true
            }
        }
    }

    #[cfg(target_family = "wasm")]
    fn prepare_code_index_for_search(
        &self,
        tab: SearchTab,
        _is_shared_session_viewer: bool,
        _ctx: &mut ViewContext<Self>,
    ) -> bool {
        tab != SearchTab::All
    }

    /// The project the code index should cover for the active tab: its locked project root, else
    /// the repository of the active session, else the session's directory, which (the `true`) must
    /// hold build files to count.
    #[cfg(not(target_family = "wasm"))]
    fn code_index_project_root(&self, ctx: &mut ViewContext<Self>) -> (Option<PathBuf>, bool) {
        let window_id = ctx.window_id();
        let mut root = self
            .active_tab_pane_group()
            .as_ref(ctx)
            .project_root()
            .primary()
            .map(Path::to_path_buf);
        if root.is_none() {
            root = crate::search::files::model::FileSearchModel::as_ref(ctx).repo_root(ctx);
        }
        let require_build_files = root.is_none();
        if root.is_none() {
            root = ActiveSession::as_ref(ctx)
                .path_if_local(window_id)
                .map(Path::to_path_buf);
        }
        (root, require_build_files)
    }
}
