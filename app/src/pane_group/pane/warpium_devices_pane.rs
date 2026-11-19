//! Warpium: the Devices pane (see [`crate::warpium::emulator`]).

use warpui::{AppContext, ModelHandle, SingletonEntity, View, ViewContext, ViewHandle};

use super::view::PaneView;
use super::{
    DetachType, PaneConfiguration, PaneContent, PaneGroup, PaneId, ShareableLink,
    ShareableLinkError,
};
use crate::app_state::LeafContents;
use crate::warpium::emulator::{DevicesPaneManager, DevicesView, DevicesViewEvent};
use crate::workspace::PaneViewLocator;

pub struct DevicesPane {
    view: ViewHandle<PaneView<DevicesView>>,
    pane_configuration: ModelHandle<PaneConfiguration>,
}

impl DevicesPane {
    fn from_view(devices_view: ViewHandle<DevicesView>, ctx: &mut AppContext) -> Self {
        let pane_configuration = devices_view.as_ref(ctx).pane_configuration();
        let view = ctx.add_typed_action_view(devices_view.window_id(ctx), |ctx| {
            let pane_id = PaneId::from_warpium_devices_pane_ctx(ctx);
            PaneView::new(pane_id, devices_view, (), pane_configuration.clone(), ctx)
        });
        Self {
            view,
            pane_configuration,
        }
    }

    pub(crate) fn new<V: View>(ctx: &mut ViewContext<V>) -> Self {
        let view = ctx.add_typed_action_view(DevicesView::new);
        Self::from_view(view, ctx)
    }

    pub(crate) fn devices_view(&self, ctx: &AppContext) -> ViewHandle<DevicesView> {
        self.view.as_ref(ctx).child(ctx)
    }
}

impl PaneContent for DevicesPane {
    fn id(&self) -> PaneId {
        PaneId::from_warpium_devices_pane_view(&self.view)
    }

    fn attach(
        &self,
        _group: &PaneGroup,
        focus_handle: crate::pane_group::focus_state::PaneFocusHandle,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        self.view
            .update(ctx, |view, ctx| view.set_focus_handle(focus_handle, ctx));

        let devices_view = self.devices_view(ctx);
        let pane_id = self.id();
        let pane_group_id = ctx.view_id();
        let window_id = ctx.window_id();

        ctx.subscribe_to_view(&devices_view, move |pane_group, _, event, ctx| {
            let DevicesViewEvent::Pane(pane_event) = event;
            pane_group.handle_pane_event(pane_id, pane_event, ctx)
        });
        ctx.subscribe_to_view(&self.view, move |group, _, event, ctx| {
            group.handle_pane_view_event(pane_id, event, ctx);
        });

        DevicesPaneManager::handle(ctx).update(ctx, |manager, _| {
            manager.register_pane(
                window_id,
                PaneViewLocator {
                    pane_group_id,
                    pane_id,
                },
            );
        });
    }

    fn detach(
        &self,
        _group: &PaneGroup,
        detach_type: DetachType,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        let devices_view = self.devices_view(ctx);
        ctx.unsubscribe_to_view(&devices_view);
        ctx.unsubscribe_to_view(&self.view);

        let window_id = ctx.window_id();
        DevicesPaneManager::handle(ctx).update(ctx, |manager, _| {
            manager.deregister_pane(&window_id);
        });
        match detach_type {
            DetachType::Closed => {
                devices_view.update(ctx, |view, ctx| view.stop_all(ctx));
            }
            // A pane hidden for an undoable close isn't painted, so its streams pause on their
            // own; a moved pane keeps streaming.
            DetachType::HiddenForClose | DetachType::Moved => {}
        }
    }

    fn snapshot(&self, _app: &AppContext) -> LeafContents {
        LeafContents::WarpiumDevices
    }

    fn has_application_focus(&self, ctx: &mut ViewContext<PaneGroup>) -> bool {
        self.view.is_self_or_child_focused(ctx)
    }

    fn focus(&self, ctx: &mut ViewContext<PaneGroup>) {
        self.devices_view(ctx)
            .update(ctx, |view, ctx| view.focus(ctx));
    }

    fn shareable_link(
        &self,
        _ctx: &mut ViewContext<PaneGroup>,
    ) -> Result<ShareableLink, ShareableLinkError> {
        Ok(ShareableLink::Base)
    }

    fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    fn is_pane_being_dragged(&self, ctx: &AppContext) -> bool {
        self.view.as_ref(ctx).is_being_dragged()
    }
}
