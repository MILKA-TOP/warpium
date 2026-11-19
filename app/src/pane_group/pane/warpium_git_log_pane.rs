//! Warpium: the Git Log pane (see [`crate::warpium::git_log`]).

use warpui::{AppContext, ModelHandle, SingletonEntity, View, ViewContext, ViewHandle};

use super::view::PaneView;
use super::{
    DetachType, PaneConfiguration, PaneContent, PaneGroup, PaneId, ShareableLink,
    ShareableLinkError,
};
use crate::app_state::LeafContents;
use crate::warpium::git_log::{GitLogPaneManager, GitLogView, GitLogViewEvent};
use crate::workspace::PaneViewLocator;

pub struct GitLogPane {
    view: ViewHandle<PaneView<GitLogView>>,
    pane_configuration: ModelHandle<PaneConfiguration>,
}

impl GitLogPane {
    pub(crate) fn new<V: View>(ctx: &mut ViewContext<V>) -> Self {
        let git_log_view = ctx.add_typed_action_view(GitLogView::new);
        let pane_configuration = git_log_view.as_ref(ctx).pane_configuration();
        let view = ctx.add_typed_action_view(|ctx| {
            let pane_id = PaneId::from_warpium_git_log_pane_ctx(ctx);
            PaneView::new(pane_id, git_log_view, (), pane_configuration.clone(), ctx)
        });
        Self {
            view,
            pane_configuration,
        }
    }

    pub(crate) fn git_log_view(&self, ctx: &AppContext) -> ViewHandle<GitLogView> {
        self.view.as_ref(ctx).child(ctx)
    }
}

impl PaneContent for GitLogPane {
    fn id(&self) -> PaneId {
        PaneId::from_warpium_git_log_pane_view(&self.view)
    }

    fn attach(
        &self,
        _group: &PaneGroup,
        focus_handle: crate::pane_group::focus_state::PaneFocusHandle,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        self.view
            .update(ctx, |view, ctx| view.set_focus_handle(focus_handle, ctx));

        let git_log_view = self.git_log_view(ctx);
        let pane_id = self.id();
        let pane_group_id = ctx.view_id();
        let window_id = ctx.window_id();

        ctx.subscribe_to_view(&git_log_view, move |pane_group, _, event, ctx| {
            let GitLogViewEvent::Pane(pane_event) = event;
            pane_group.handle_pane_event(pane_id, pane_event, ctx)
        });
        ctx.subscribe_to_view(&self.view, move |group, _, event, ctx| {
            group.handle_pane_view_event(pane_id, event, ctx);
        });

        GitLogPaneManager::handle(ctx).update(ctx, |manager, _| {
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
        let git_log_view = self.git_log_view(ctx);
        ctx.unsubscribe_to_view(&git_log_view);
        ctx.unsubscribe_to_view(&self.view);

        let window_id = ctx.window_id();
        let pane_id = self.id();
        GitLogPaneManager::handle(ctx).update(ctx, |manager, _| {
            manager.deregister_pane(&window_id, pane_id);
        });
    }

    fn snapshot(&self, _app: &AppContext) -> LeafContents {
        LeafContents::WarpiumGitLog
    }

    fn has_application_focus(&self, ctx: &mut ViewContext<PaneGroup>) -> bool {
        self.view.is_self_or_child_focused(ctx)
    }

    fn focus(&self, ctx: &mut ViewContext<PaneGroup>) {
        self.git_log_view(ctx)
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
