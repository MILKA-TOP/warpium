//! Warpium: the workspace side of the bottom panel. Its window-wide actions act on the active
//! tab's panel, sub-tabs move between tabs' panels, restored tabs get their panel back and Logcat
//! opens in the panel.

use std::collections::HashMap;

use warp_errors::report_if_error;
use warpium_bottom_panel::DockOrigin;
use warpui::{ViewContext, ViewHandle};

use super::Workspace;
use crate::ai::blocklist::SerializedBlockListItem;
use crate::app_state::PaneUuid;
use crate::pane_group::{BottomPanelAction, BottomPanelEvent, BottomPanelTabTarget, PaneGroup};
use crate::warpium::sync_settings::WarpiumBottomPanelSettings;

impl Workspace {
    pub(super) fn handle_warpium_bottom_panel_action(
        &mut self,
        action: &BottomPanelAction,
        ctx: &mut ViewContext<Self>,
    ) {
        self.active_tab_pane_group().update(ctx, |pane_group, ctx| {
            pane_group.handle_warpium_bottom_panel_action(action, ctx)
        });
    }

    pub(super) fn toggle_warpium_bottom_panel_setting(&mut self, ctx: &mut ViewContext<Self>) {
        use warpui::SingletonEntity as _;

        WarpiumBottomPanelSettings::handle(ctx).update(ctx, |settings, ctx| {
            report_if_error!(settings.bottom_panel.toggle_and_save_value(ctx));
        });
        ctx.notify();
    }

    pub(super) fn handle_warpium_bottom_panel_event(
        &mut self,
        source: &ViewHandle<PaneGroup>,
        event: &BottomPanelEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            BottomPanelEvent::MoveToTab { pane_id, target } => {
                self.move_docked_pane_to_tab(source, *pane_id, *target, ctx)
            }
        }
    }

    /// Moves a docked sub-tab to another tab's panel, keeping its process, title and origin, or
    /// to a new tab as its only pane; the target tab becomes active.
    fn move_docked_pane_to_tab(
        &mut self,
        source: &ViewHandle<PaneGroup>,
        pane_id: crate::pane_group::PaneId,
        target: BottomPanelTabTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        let target_tab = match target {
            BottomPanelTabTarget::Tab(index) => match self.tabs.get(index) {
                Some(tab) if tab.pane_group.id() != source.id() => Some(tab.pane_group.clone()),
                // Dropped on its own tab: nothing moves.
                Some(_) => return,
                None => {
                    log::warn!("[Bottom panel] No tab to move the sub-tab to: index={index}");
                    return;
                }
            },
            BottomPanelTabTarget::NewTab => None,
        };
        let Some((pane, entry)) = source.update(ctx, |pane_group, ctx| {
            pane_group.take_docked_pane_for_move(pane_id, ctx)
        }) else {
            return;
        };
        match target_tab {
            Some(target_tab) => {
                let target_index = self
                    .tabs
                    .iter()
                    .position(|tab| tab.pane_group.id() == target_tab.id());
                target_tab.update(ctx, |pane_group, ctx| {
                    pane_group.adopt_pane_into_bottom_panel(pane, entry.origin, entry.title, ctx);
                });
                if let Some(index) = target_index {
                    self.activate_tab(index, ctx);
                }
            }
            None => self.add_tab_from_existing_pane(pane, self.tabs.len(), None, ctx),
        }
        ctx.notify();
    }

    /// Gives a restored tab its saved bottom panel back.
    pub(super) fn restore_warpium_bottom_panel_for_tab(
        &mut self,
        pane_group: &ViewHandle<PaneGroup>,
        snapshot: Option<&warpium_bottom_panel::PanelSnapshot>,
        block_lists: &HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(snapshot) = snapshot else {
            return;
        };
        pane_group.update(ctx, |pane_group, ctx| {
            pane_group.restore_warpium_bottom_panel(snapshot, block_lists, ctx)
        });
    }

    /// With the bottom panel on, Logcat lives in the active tab's panel. The window keeps a
    /// single Logcat pane (one adb stream per window): opening it from another tab moves it into
    /// this tab's panel, unless that would close the tab it is in (then it is shown there).
    /// Returns `None` when the panel is off, so the caller opens it in a split as before.
    #[cfg(not(target_family = "wasm"))]
    pub(super) fn open_android_logcat_in_bottom_panel(
        &mut self,
        project_root: Option<std::path::PathBuf>,
        focus: bool,
        ctx: &mut ViewContext<Self>,
    ) -> Option<Option<ViewHandle<crate::warpium::logcat::LogcatView>>> {
        use warpui::SingletonEntity as _;

        use crate::pane_group::LogcatPane;
        use crate::warpium::logcat::{LogcatPaneManager, default_package};

        if !crate::warpium::bottom_panel::is_enabled(ctx) {
            return None;
        }
        let active = self.active_tab_pane_group().clone();
        let logcat_view = |pane_group: &ViewHandle<PaneGroup>,
                           pane_id: crate::pane_group::PaneId,
                           ctx: &ViewContext<Self>| {
            pane_group
                .as_ref(ctx)
                .downcast_pane_by_id::<LogcatPane>(pane_id)
                .map(|pane| pane.logcat_view(ctx))
        };

        if let Some(locator) = LogcatPaneManager::as_ref(ctx).find_pane(ctx.window_id()) {
            let source = self
                .tabs
                .iter()
                .find(|tab| tab.pane_group.id() == locator.pane_group_id)
                .map(|tab| tab.pane_group.clone());
            let Some(source) = source else {
                log::warn!("[Bottom panel] The window's Logcat pane is in no tab");
                return Some(None);
            };
            let pane_id = locator.pane_id;
            let in_active_tab = source.id() == active.id();
            if in_active_tab || !source.as_ref(ctx).can_move_pane_out_keeping_tab(pane_id) {
                if in_active_tab && source.as_ref(ctx).is_pane_docked(pane_id) {
                    source.update(ctx, |pane_group, ctx| {
                        pane_group.reveal_docked_pane(pane_id, focus, ctx)
                    });
                } else if focus || !in_active_tab {
                    self.focus_pane(locator, ctx);
                }
                return Some(logcat_view(&source, pane_id, ctx));
            }
            let pane = if source.as_ref(ctx).is_pane_docked(pane_id) {
                source
                    .update(ctx, |pane_group, ctx| {
                        pane_group.take_docked_pane_for_move(pane_id, ctx)
                    })
                    .map(|(pane, _)| pane)
            } else {
                source.update(ctx, |pane_group, ctx| {
                    pane_group.remove_pane_for_move(&pane_id, ctx)
                })
            };
            let Some(pane) = pane else {
                return Some(None);
            };
            let new_pane_id = active.update(ctx, |pane_group, ctx| {
                let new_pane_id =
                    pane_group.adopt_pane_into_bottom_panel(pane, DockOrigin::Logcat, None, ctx);
                if let Some(new_pane_id) = new_pane_id {
                    pane_group.reveal_docked_pane(new_pane_id, focus, ctx);
                }
                new_pane_id
            });
            return Some(new_pane_id.and_then(|pane_id| logcat_view(&active, pane_id, ctx)));
        }

        let package = project_root.as_deref().and_then(default_package);
        let pane = LogcatPane::new(project_root, package, ctx);
        let view = pane.logcat_view(ctx);
        active.update(ctx, |pane_group, ctx| {
            let pane_id = pane_group.adopt_pane_into_bottom_panel(
                Box::new(pane),
                DockOrigin::Logcat,
                None,
                ctx,
            );
            if let Some(pane_id) = pane_id {
                pane_group.reveal_docked_pane(pane_id, focus, ctx);
            }
        });
        Some(Some(view))
    }
}
