//! Warpium: opening the Logcat pane, the Android palette commands and running Android apps.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use warpium_android::project::AndroidAppModule;
use warpium_android::run::LaunchTarget;
use warpium_project_toolbar::{CommandAction, CommandTarget};
use warpui::{SingletonEntity as _, ViewContext};

use super::Workspace;
use crate::pane_group::{Direction, LogcatPane};
use crate::warpium::android::AndroidModel;
use crate::warpium::logcat::{LogcatMenu, LogcatPaneManager, LogcatView, default_package};

/// The build variant runs install. Flavored projects need a toolbar command for now.
const RUN_VARIANT: &str = "debug";

impl Workspace {
    /// The project the Android tools work on: the project toolbar's project.
    fn android_project_root(&self, ctx: &ViewContext<Self>) -> Option<PathBuf> {
        self.project_toolbar
            .as_ref(ctx)
            .project_root()
            .map(Path::to_path_buf)
    }

    /// Points Warpium's MCP servers at the current project before registering or installing them.
    pub(super) fn update_agent_mcp_project(&self, ctx: &mut ViewContext<Self>) {
        let root = self.android_project_root(ctx);
        crate::warpium::agent_mcp::AgentMcpModel::handle(ctx)
            .update(ctx, |model, ctx| model.set_project_root(root, ctx));
    }

    /// Opens the Logcat pane below the active pane, or shows the window's existing one, and
    /// optionally opens one of its menus. `focus` moves the focus to it.
    pub(super) fn open_android_logcat(
        &mut self,
        menu: Option<LogcatMenu>,
        focus: bool,
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
                if focus || menu.is_some() {
                    self.focus_pane(locator, ctx);
                }
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
                        focus || menu.is_some(),
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

    /// Runs `module` on the selected device: `./gradlew :<module>:installDebug` visibly in the
    /// active session (with `ANDROID_SERIAL` set), then, once the device reports the new
    /// install, starts `launch` (nothing when `None`). Logcat follows the app.
    pub(super) fn run_android_app(
        &mut self,
        root: &Path,
        module: &AndroidAppModule,
        launch: Option<&LaunchTarget>,
        ctx: &mut ViewContext<Self>,
    ) {
        let serial = AndroidModel::as_ref(ctx)
            .selected_device()
            .filter(|device| device.entry.state.is_online())
            .map(|device| device.serial().to_owned());
        let Some(serial) = serial else {
            self.show_project_toolbar_error(
                "Select an Android device or start an emulator to run the app.".to_owned(),
                ctx,
            );
            self.project_toolbar
                .update(ctx, |toolbar, ctx| toolbar.open_device_menu(ctx));
            return;
        };
        let package = module.package_for_variant(RUN_VARIANT);
        // Waiting for the install starts first, so it reads the install time before the build
        // can replace it.
        if let (Some(package), Some(launch)) = (&package, launch) {
            let (serial, package, launch) = (serial.clone(), package.clone(), launch.clone());
            let metadata = Some(module.output_metadata_path(RUN_VARIANT));
            AndroidModel::handle(ctx).update(ctx, |android, ctx| {
                android.launch_after_install(serial, package, metadata, launch, ctx)
            });
        }
        let gradle = if root.join("gradlew").is_file() {
            "./gradlew"
        } else {
            "gradle"
        };
        let command = CommandAction {
            command: format!("{gradle} {}", module.install_task(RUN_VARIANT)),
            target: CommandTarget::Current,
            cwd: None,
            env: BTreeMap::from([("ANDROID_SERIAL".to_owned(), serial)]),
        };
        let title = format!("Run {}", module.display_name());
        self.run_project_toolbar_command(root, &command, &title, ctx);

        let Some(package) = package else {
            if launch.is_none() {
                return;
            }
            self.show_project_toolbar_error(
                format!(
                    "Couldn't find the application id of {}; it is installed but not started.",
                    module.display_name()
                ),
                ctx,
            );
            return;
        };
        if let Some(logcat) = self.open_android_logcat(None, false, ctx) {
            logcat.update(ctx, |logcat, ctx| logcat.set_package(package, ctx));
        }
    }
}
