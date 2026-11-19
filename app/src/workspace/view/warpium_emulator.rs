//! Warpium: opening the Devices pane, which shows running Android emulators.

use warpui::{SingletonEntity as _, ViewContext};

use super::Workspace;
use crate::pane_group::{DevicesPane, Direction};
use crate::warpium::android::AndroidModel;
use crate::warpium::emulator::DevicesPaneManager;

impl Workspace {
    /// Opens the Devices pane to the right of the active pane, or focuses the window's existing
    /// one, and selects the emulator with adb serial `serial` when given.
    pub(super) fn show_running_emulator(
        &mut self,
        serial: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        if !crate::warpium::emulator::is_enabled(ctx) {
            return;
        }
        // The device tracker tells the pane when emulators start or stop.
        let project_root = self
            .project_toolbar
            .as_ref(ctx)
            .project_root()
            .map(std::path::Path::to_path_buf);
        AndroidModel::handle(ctx).update(ctx, |android, ctx| {
            android.ensure_started(project_root.as_deref(), ctx)
        });

        let existing = DevicesPaneManager::as_ref(ctx).find_pane(ctx.window_id());
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
                                .downcast_pane_by_id::<DevicesPane>(locator.pane_id)
                                .map(|pane| pane.devices_view(ctx))
                        })
                    });
                self.focus_pane(locator, ctx);
                view
            }
            None => {
                let pane = DevicesPane::new(ctx);
                let view = pane.devices_view(ctx);
                self.active_tab_pane_group().update(ctx, |pane_group, ctx| {
                    pane_group.add_pane_with_direction(Direction::Right, pane, true, ctx);
                });
                Some(view)
            }
        };
        if let Some(view) = view {
            view.update(ctx, |view, ctx| view.show(serial, ctx));
        }
    }

    #[cfg(debug_assertions)]
    pub(super) fn start_fake_emulator(
        &mut self,
        kind: crate::warpium::FakeEmulatorKind,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(serial) = crate::warpium::emulator::debug_fakes::start(kind, ctx) {
            self.show_running_emulator(Some(serial), ctx);
        }
    }

    #[cfg(debug_assertions)]
    pub(super) fn stop_fake_emulators(&mut self, ctx: &mut ViewContext<Self>) {
        crate::warpium::emulator::debug_fakes::stop_all(ctx);
        let Some(locator) = DevicesPaneManager::as_ref(ctx).find_pane(ctx.window_id()) else {
            return;
        };
        let view = self
            .tabs
            .iter()
            .find(|tab| tab.pane_group.id() == locator.pane_group_id)
            .map(|tab| tab.pane_group.clone())
            .and_then(|pane_group| {
                pane_group.read(ctx, |pane_group, ctx| {
                    pane_group
                        .downcast_pane_by_id::<DevicesPane>(locator.pane_id)
                        .map(|pane| pane.devices_view(ctx))
                })
            });
        if let Some(view) = view {
            view.update(ctx, |view, ctx| view.show(None, ctx));
        }
    }
}
