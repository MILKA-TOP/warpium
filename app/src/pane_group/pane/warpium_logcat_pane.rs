//! Warpium: the Logcat pane (see [`crate::warpium::logcat`]).

use std::path::PathBuf;

use warpui::{AppContext, ModelHandle, SingletonEntity, View, ViewContext, ViewHandle};

use super::view::PaneView;
use super::{
    DetachType, PaneConfiguration, PaneContent, PaneGroup, PaneId, ShareableLink,
    ShareableLinkError,
};
use crate::app_state::LeafContents;
use crate::warpium::logcat::{LogcatPaneManager, LogcatView, LogcatViewEvent};
use crate::workspace::PaneViewLocator;

pub struct LogcatPane {
    view: ViewHandle<PaneView<LogcatView>>,
    pane_configuration: ModelHandle<PaneConfiguration>,
}

impl LogcatPane {
    fn from_view(logcat_view: ViewHandle<LogcatView>, ctx: &mut AppContext) -> Self {
        let pane_configuration = logcat_view.as_ref(ctx).pane_configuration();
        let view = ctx.add_typed_action_view(logcat_view.window_id(ctx), |ctx| {
            let pane_id = PaneId::from_warpium_logcat_pane_ctx(ctx);
            PaneView::new(pane_id, logcat_view, (), pane_configuration.clone(), ctx)
        });
        Self {
            view,
            pane_configuration,
        }
    }

    /// A Logcat pane for the selected device; `package` pre-fills its package filter.
    pub(crate) fn new<V: View>(
        project_root: Option<PathBuf>,
        package: Option<String>,
        ctx: &mut ViewContext<V>,
    ) -> Self {
        let view = ctx.add_typed_action_view(|ctx| LogcatView::new(project_root, package, ctx));
        Self::from_view(view, ctx)
    }

    pub(crate) fn logcat_view(&self, ctx: &AppContext) -> ViewHandle<LogcatView> {
        self.view.as_ref(ctx).child(ctx)
    }
}

impl PaneContent for LogcatPane {
    fn id(&self) -> PaneId {
        PaneId::from_warpium_logcat_pane_view(&self.view)
    }

    fn attach(
        &self,
        _group: &PaneGroup,
        focus_handle: crate::pane_group::focus_state::PaneFocusHandle,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        self.view
            .update(ctx, |view, ctx| view.set_focus_handle(focus_handle, ctx));

        let logcat_view = self.logcat_view(ctx);
        let pane_id = self.id();
        let pane_group_id = ctx.view_id();
        let window_id = ctx.window_id();

        ctx.subscribe_to_view(&logcat_view, move |pane_group, _, event, ctx| {
            let LogcatViewEvent::Pane(pane_event) = event;
            pane_group.handle_pane_event(pane_id, pane_event, ctx)
        });
        ctx.subscribe_to_view(&self.view, move |group, _, event, ctx| {
            group.handle_pane_view_event(pane_id, event, ctx);
        });

        LogcatPaneManager::handle(ctx).update(ctx, |manager, _| {
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
        _detach_type: DetachType,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        let logcat_view = self.logcat_view(ctx);
        ctx.unsubscribe_to_view(&logcat_view);
        ctx.unsubscribe_to_view(&self.view);

        let window_id = ctx.window_id();
        LogcatPaneManager::handle(ctx).update(ctx, |manager, _| {
            manager.deregister_pane(&window_id);
        });
    }

    fn snapshot(&self, _app: &AppContext) -> LeafContents {
        LeafContents::WarpiumLogcat
    }

    fn has_application_focus(&self, ctx: &mut ViewContext<PaneGroup>) -> bool {
        self.view.is_self_or_child_focused(ctx)
    }

    fn focus(&self, ctx: &mut ViewContext<PaneGroup>) {
        self.logcat_view(ctx)
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
