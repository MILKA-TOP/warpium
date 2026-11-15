//! Warpium: opening the Logcat pane and the Android palette commands.

use std::path::{Path, PathBuf};

use warpui::{SingletonEntity as _, ViewContext};

use super::Workspace;
use crate::pane_group::{Direction, LogcatPane};
use crate::warpium::android::AndroidModel;
use crate::warpium::logcat::{LogcatMenu, LogcatPaneManager, LogcatView, default_package};

impl Workspace {
    /// The project the Android tools work on: the project toolbar's project.
    fn android_project_root(&self, ctx: &ViewContext<Self>) -> Option<PathBuf> {
        self.project_toolbar
            .as_ref(ctx)
            .project_root()
            .map(Path::to_path_buf)
    }

    /// Opens the Logcat pane below the active pane, or focuses the window's existing one, and
    /// optionally opens one of its menus.
    pub(super) fn open_android_logcat(
        &mut self,
        menu: Option<LogcatMenu>,
        ctx: &mut ViewContext<Self>,
    ) -> Option<warpui::ViewHandle<LogcatView>> {
        if !crate::warpium::android::is_enabled(ctx) {
            return None;
        }
        let project_root = self.android_project_root(ctx);
        AndroidModel::handle(ctx).update(ctx, |android, ctx| {
            android.ensure_started(project_root.as_deref(), ctx)
        });

        let existing = LogcatPaneManager::as_ref(ctx).find_pane(ctx.window_id());
        let view = match existing {
            Some(locator) => {
                let view = self
                    .tabs
                    .iter()
                    .find(|tab| tab.pane_group.id() == locator.pane_group_id)
                    .map(|tab| tab.pane_group.clone())
                    .and_then(|pane_group| {
                        pane_group.read(ctx, |pane_group, ctx| {
                            pane_group
                                .downcast_pane_by_id::<LogcatPane>(locator.pane_id)
                                .map(|pane| pane.logcat_view(ctx))
                        })
                    });
                self.focus_pane(locator, ctx);
                view
            }
            None => {
                let package = project_root.as_deref().and_then(default_package);
                let pane = LogcatPane::new(project_root, package, ctx);
                let view = pane.logcat_view(ctx);
                self.active_tab_pane_group().update(ctx, |pane_group, ctx| {
                    pane_group.add_pane_with_direction(
                        Direction::Down,
                        pane,
                        true, /* focus_new_pane */
                        ctx,
                    );
                });
                Some(view)
            }
        };
        if let (Some(view), Some(menu)) = (&view, menu) {
            view.update(ctx, |view, ctx| view.show_menu(menu, ctx));
        }
        view
    }
}
