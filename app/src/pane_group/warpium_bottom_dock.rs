//! Warpium: the bottom panel, a per-tab dock below the split layout (roadmap item 14).
//!
//! A docked pane leaves the layout tree but stays in `pane_contents`, so its view, terminal
//! manager and PTY keep running, like a child agent pane that is off-tree. The dock only records
//! which panes are docked and which one it shows. Everything that walks the layout tree (focus
//! navigation, splits, resizing, drag and drop) is kept away from docked panes here, and every
//! path that would empty the layout (closing or moving away its last pane) first opens a fresh
//! terminal so the tab, and the processes in its dock, survive.
//!
//! Docked panes are not part of the session snapshot yet, so they aren't restored on relaunch.

use std::collections::{HashMap, HashSet};

use warpium_bottom_panel::{
    Axis, BottomDock, LayoutNode, RestoreHint, Side, layout_anchor, restore_hint,
};
use warpui::elements::{
    Border, Clipped, ConstrainedBox, Container, CrossAxisAlignment, DragBarSide, Element, Flex,
    Hoverable, MainAxisSize, MouseStateHandle, ParentElement, Resizable, ResizableStateHandle,
    Shrinkable, Text, resizable_state_handle,
};
use warpui::keymap::{Context, EditableBinding};
use warpui::platform::Cursor;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::components::UiComponent;
use warpui::{AppContext, SingletonEntity as _, ViewContext};

use super::pane::view::PaneViewEvent;
use super::{
    Direction, PaneConfigurationEvent, PaneData, PaneGroup, PaneGroupAction, PaneId, PaneNode,
    SplitDirection,
};
use crate::appearance::Appearance;
use crate::features::FeatureFlag;
use crate::terminal::session_settings::{NewSessionSource, SessionSettings};
use crate::terminal::shared_session::IsSharedSessionCreator;
use crate::ui_components::buttons::icon_button;
use crate::ui_components::icons::Icon;

/// Keymap context set while the focused pane is docked.
const FOCUSED_PANE_DOCKED: &str = "PaneGroup_FocusedPaneDocked";
/// Keymap context set while the focused pane can move to the bottom panel.
const FOCUSED_PANE_DOCKABLE: &str = "PaneGroup_FocusedPaneDockable";

const DEFAULT_HEIGHT: f32 = 260.;
const MIN_HEIGHT: f32 = 100.;
/// Height the layout keeps above the panel when it is dragged up.
const MIN_LAYOUT_HEIGHT: f32 = 160.;
const TAB_STRIP_HEIGHT: f32 = 28.;
const TAB_MAX_WIDTH: f32 = 200.;
const TAB_FONT_SIZE: f32 = 12.;

/// Moves a pane between the layout and the bottom panel, from a pane's own menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomPanelPaneRequest {
    MoveToBottomPanel,
    RestoreToLayout,
}

#[derive(Debug, Clone)]
pub enum BottomPanelAction {
    /// Shows or hides the panel; opens a new terminal in it when it is empty.
    Toggle,
    /// Hides the panel.
    Hide,
    MoveFocusedPane,
    RestoreFocusedPane,
    /// Shows and focuses the sub-tab of a docked pane.
    Select(PaneId),
    Restore(PaneId),
}

/// The bottom panel of one tab.
pub(super) struct WarpiumBottomDock {
    model: BottomDock<PaneId>,
    height: ResizableStateHandle,
    tab_mouse_states: HashMap<PaneId, TabMouseStates>,
    restore_button: MouseStateHandle,
    hide_button: MouseStateHandle,
    /// Docked panes whose title changes re-render the tab strip.
    title_subscriptions: HashSet<PaneId>,
}

#[derive(Default)]
struct TabMouseStates {
    tab: MouseStateHandle,
    close: MouseStateHandle,
}

impl Default for WarpiumBottomDock {
    fn default() -> Self {
        Self {
            model: BottomDock::default(),
            height: resizable_state_handle(DEFAULT_HEIGHT),
            tab_mouse_states: HashMap::new(),
            restore_button: MouseStateHandle::default(),
            hide_button: MouseStateHandle::default(),
            title_subscriptions: HashSet::new(),
        }
    }
}

pub(super) fn init_bindings(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_editable_bindings([
        EditableBinding::new(
            "pane_group:toggle_bottom_panel",
            "Toggle bottom panel",
            PaneGroupAction::WarpiumBottomPanel(BottomPanelAction::Toggle),
        )
        .with_context_predicate(id!("PaneGroup") & !id!("PaneGroup_PaneDragging"))
        .with_enabled(|| FeatureFlag::WarpiumBottomPanel.is_enabled())
        .with_mac_key_binding("cmd-j"),
        EditableBinding::new(
            "pane_group:move_pane_to_bottom_panel",
            "Move pane to bottom panel",
            PaneGroupAction::WarpiumBottomPanel(BottomPanelAction::MoveFocusedPane),
        )
        .with_context_predicate(id!("PaneGroup") & id!(FOCUSED_PANE_DOCKABLE))
        .with_enabled(|| FeatureFlag::WarpiumBottomPanel.is_enabled()),
        EditableBinding::new(
            "pane_group:restore_pane_from_bottom_panel",
            "Restore pane to layout",
            PaneGroupAction::WarpiumBottomPanel(BottomPanelAction::RestoreFocusedPane),
        )
        .with_context_predicate(id!("PaneGroup") & id!(FOCUSED_PANE_DOCKED))
        .with_enabled(|| FeatureFlag::WarpiumBottomPanel.is_enabled()),
    ]);
}

impl PaneGroup {
    pub(super) fn is_pane_docked(&self, pane_id: PaneId) -> bool {
        self.warpium_bottom_dock.model.contains(pane_id)
    }

    /// Whether the pane can move to the bottom panel: a visible terminal pane of the layout that
    /// no other mechanism (child agents, temporary swaps) manages.
    fn is_pane_dockable(&self, pane_id: PaneId) -> bool {
        FeatureFlag::WarpiumBottomPanel.is_enabled()
            && pane_id.as_terminal_pane_id().is_some()
            && self.pane_contents.contains_key(&pane_id)
            && self.panes.is_pane_in_tree(pane_id)
            && !self.panes.is_pane_hidden(&pane_id)
            && !self.is_child_agent_pane(pane_id)
            && !self.panes.is_temporary_replacement(pane_id)
            && self.panes.replacement_pane_for_original(pane_id).is_none()
    }

    /// The layout pane to act on instead of a docked one: the most recently focused visible pane
    /// of the layout other than `excluded`.
    fn warpium_layout_anchor(&self, excluded: Option<PaneId>) -> Option<PaneId> {
        let visible: Vec<PaneId> = self
            .panes
            .visible_pane_ids()
            .into_iter()
            .filter(|id| Some(*id) != excluded)
            .collect();
        layout_anchor(&self.pane_history, &visible)
    }

    /// Hook for adding a pane next to `base_pane_id`: a docked base is replaced by the layout
    /// pane the user last focused, since docked panes aren't in the tree to split.
    pub(super) fn warpium_split_base(&self, base_pane_id: PaneId) -> PaneId {
        if !self.is_pane_docked(base_pane_id) {
            return base_pane_id;
        }
        self.warpium_layout_anchor(None).unwrap_or(base_pane_id)
    }

    /// Hook for `close_pane`: closes a docked pane (its shell exited or the user closed its
    /// sub-tab) without touching the layout or the tab. Returns false for other panes.
    pub(super) fn warpium_close_docked_pane(
        &mut self,
        pane_id: PaneId,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if !self.is_pane_docked(pane_id) {
            return false;
        }
        let was_focused = self.focused_pane_id(ctx) == pane_id;
        self.undock_pane(pane_id, ctx);
        if was_focused {
            self.focus_after_leaving_dock(ctx);
        }
        if Some(pane_id) == self.active_session_id(ctx).map(Into::into) {
            let new_active_session = self.choose_active_session(pane_id);
            self.focus_state.update(ctx, |focus_state, ctx| {
                focus_state.set_active_session(new_active_session, ctx);
            });
        }
        self.remove_from_pane_history(pane_id);
        // Dropping the pane's contents drops its terminal manager, which ends the PTY.
        self.discard_pane(pane_id, ctx);
        self.handle_pane_count_change(ctx);
        ctx.emit(super::Event::TerminalViewStateChanged);
        ctx.emit(super::Event::AppStateChanged);
        true
    }

    /// Hook for paths that take `pane_id` out of the layout for good (closing it, moving it to
    /// another tab): when it is the layout's last visible pane and the dock isn't empty, a fresh
    /// terminal takes its place so the tab and the docked processes stay alive.
    pub(super) fn warpium_keep_layout_for_removal(
        &mut self,
        pane_id: PaneId,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.warpium_bottom_dock.model.is_empty() {
            return;
        }
        if self.is_pane_docked(pane_id) {
            // Docked panes never reach the move paths; keep the dock consistent if one does.
            self.undock_pane(pane_id, ctx);
            return;
        }
        if !self.panes.is_pane_in_tree(pane_id) || self.is_pane_hidden_for_close(pane_id) {
            return;
        }
        let layout_keeps_a_pane = self
            .panes
            .visible_pane_ids()
            .into_iter()
            .any(|id| id != pane_id);
        if !layout_keeps_a_pane {
            self.open_fresh_terminal_next_to(pane_id, ctx);
        }
    }

    /// Hook for `focus_pane`: focusing a docked pane shows the panel on its sub-tab.
    pub(super) fn warpium_on_pane_focused(&mut self, pane_id: PaneId) {
        self.warpium_bottom_dock.model.select(pane_id);
    }

    /// Hook for `update_pane_history`: docked panes stay out of the history, which picks the
    /// layout pane to focus after a close.
    pub(super) fn warpium_skips_pane_history(&self, pane_id: PaneId) -> bool {
        self.is_pane_docked(pane_id)
    }

    /// Hook for `handle_focus_change`, which only scans the layout: records focus that moved into
    /// a docked pane. Returns true if it did.
    pub(super) fn warpium_focus_docked_pane_with_app_focus(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(pane_id) = self.warpium_bottom_dock.model.visible_pane() else {
            return false;
        };
        let has_focus = self
            .pane_contents
            .get(&pane_id)
            .is_some_and(|pane| pane.as_pane().has_application_focus(ctx));
        if !has_focus {
            return false;
        }
        // Only the pane group's state changes; application focus is already there.
        self.focus_pane(pane_id, false, ctx);
        ctx.emit(super::Event::PaneFocused);
        true
    }

    /// Hook for pane view events: docked panes can't be dragged (they aren't in the tree), and
    /// panes can't be dropped onto them.
    pub(super) fn warpium_ignores_pane_view_event(
        &self,
        pane_id: PaneId,
        event: &PaneViewEvent,
    ) -> bool {
        match event {
            PaneViewEvent::MovePaneWithinPaneGroup { target_id, .. } => {
                self.is_pane_docked(pane_id) || self.is_pane_docked(*target_id)
            }
            PaneViewEvent::DroppedOnTabBar { .. } | PaneViewEvent::DraggedOntoTabBar { .. } => {
                self.is_pane_docked(pane_id)
            }
            PaneViewEvent::PaneDraggedOutsideTabBarOrPaneGroup
            | PaneViewEvent::PaneDragEnded
            | PaneViewEvent::PaneHeaderClicked => false,
        }
    }

    /// Hook for `handle_action`: layout navigation and keyboard resizing from a docked pane act on
    /// the layout instead (navigating returns focus to it). Returns true if it handled `action`.
    pub(super) fn warpium_handle_action_for_docked_focus(
        &mut self,
        action: &PaneGroupAction,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if !self.is_pane_docked(self.focused_pane_id(ctx)) {
            return false;
        }
        match action {
            PaneGroupAction::NavigatePrev
            | PaneGroupAction::NavigateNext
            | PaneGroupAction::NavigateLeft
            | PaneGroupAction::NavigateRight
            | PaneGroupAction::NavigateUp
            | PaneGroupAction::NavigateDown => {
                self.focus_after_leaving_dock(ctx);
                true
            }
            PaneGroupAction::ResizeLeft
            | PaneGroupAction::ResizeRight
            | PaneGroupAction::ResizeUp
            | PaneGroupAction::ResizeDown => true,
            PaneGroupAction::Add(_)
            | PaneGroupAction::Remove(_)
            | PaneGroupAction::RemoveActive
            | PaneGroupAction::Activate(..)
            | PaneGroupAction::ResizeMove(_)
            | PaneGroupAction::StartResizing(_)
            | PaneGroupAction::ResetPaneSizes(_)
            | PaneGroupAction::Move { .. }
            | PaneGroupAction::EndResizing
            | PaneGroupAction::ToggleMaximizePane
            | PaneGroupAction::HandleFocusChange
            | PaneGroupAction::FocusTerminalView(_)
            | PaneGroupAction::WarpiumBottomPanel(_) => false,
        }
    }

    pub(super) fn warpium_extend_keymap_context(&self, context: &mut Context, app: &AppContext) {
        if !FeatureFlag::WarpiumBottomPanel.is_enabled() {
            return;
        }
        let focused = self.focused_pane_id(app);
        if self.is_pane_docked(focused) {
            context.set.insert(FOCUSED_PANE_DOCKED);
        } else if self.is_pane_dockable(focused) {
            context.set.insert(FOCUSED_PANE_DOCKABLE);
        }
    }

    pub(super) fn handle_warpium_pane_request(
        &mut self,
        pane_id: PaneId,
        request: BottomPanelPaneRequest,
        ctx: &mut ViewContext<Self>,
    ) {
        match request {
            BottomPanelPaneRequest::MoveToBottomPanel => {
                self.move_pane_to_bottom_panel(pane_id, ctx);
            }
            BottomPanelPaneRequest::RestoreToLayout => {
                self.restore_pane_from_bottom_panel(pane_id, ctx);
            }
        }
    }

    pub(super) fn handle_warpium_bottom_panel_action(
        &mut self,
        action: &BottomPanelAction,
        ctx: &mut ViewContext<Self>,
    ) {
        if !FeatureFlag::WarpiumBottomPanel.is_enabled() {
            return;
        }
        match action {
            BottomPanelAction::Toggle => self.toggle_bottom_panel(ctx),
            BottomPanelAction::Hide => self.hide_bottom_panel(ctx),
            BottomPanelAction::MoveFocusedPane => {
                let pane_id = self.focused_pane_id(ctx);
                self.move_pane_to_bottom_panel(pane_id, ctx);
            }
            BottomPanelAction::RestoreFocusedPane => {
                let pane_id = self.focused_pane_id(ctx);
                self.restore_pane_from_bottom_panel(pane_id, ctx);
            }
            BottomPanelAction::Select(pane_id) => self.focus_pane_by_id(*pane_id, ctx),
            BottomPanelAction::Restore(pane_id) => {
                self.restore_pane_from_bottom_panel(*pane_id, ctx);
            }
        }
    }

    /// Moves a pane of the layout to the bottom panel; its process keeps running. When it was
    /// the layout's last visible pane, a fresh terminal takes its place. Returns whether it moved.
    pub(crate) fn move_pane_to_bottom_panel(
        &mut self,
        pane_id: PaneId,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if !self.is_pane_dockable(pane_id) {
            log::warn!("[Bottom panel] Pane can't move to the bottom panel: pane_id={pane_id:?}");
            return false;
        }
        let layout_keeps_a_pane = self
            .panes
            .visible_pane_ids()
            .into_iter()
            .any(|id| id != pane_id);
        if !layout_keeps_a_pane {
            self.open_fresh_terminal_next_to(pane_id, ctx);
        }

        let hint = layout_of(&self.panes.root, &self.panes)
            .and_then(|layout| restore_hint(&layout, pane_id));
        let was_focused = self.focused_pane_id(ctx) == pane_id;
        if !self.panes.remove(pane_id) {
            log::warn!("[Bottom panel] Pane to dock was not in the layout: pane_id={pane_id:?}");
            return false;
        }
        self.remove_from_pane_history(pane_id);
        self.dock_pane(pane_id, hint, ctx);
        if was_focused {
            self.focus_after_leaving_dock(ctx);
        }
        self.handle_pane_count_change(ctx);
        ctx.notify();
        ctx.emit(super::Event::TerminalViewStateChanged);
        ctx.emit(super::Event::AppStateChanged);
        true
    }

    /// Puts a docked pane back into the layout, where it was if that neighbor is still visible,
    /// else to the right of the layout pane last focused. Returns whether it moved.
    pub(crate) fn restore_pane_from_bottom_panel(
        &mut self,
        pane_id: PaneId,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(hint) = self.undock_pane(pane_id, ctx) else {
            return false;
        };
        let visible = self.panes.visible_pane_ids();
        let target = hint
            .filter(|hint| visible.contains(&hint.anchor))
            .map(|hint| (hint.anchor, direction_of(hint.side)))
            .or_else(|| {
                self.warpium_layout_anchor(None)
                    .map(|anchor| (anchor, Direction::Right))
            });
        let inserted =
            target.is_some_and(|(anchor, direction)| self.panes.split(anchor, pane_id, direction));
        if !inserted {
            self.panes.split_root(pane_id, Direction::Right);
        }
        self.handle_pane_count_change(ctx);
        // A pane restored from the dock while focused keeps focus, which `focus_pane` treats as
        // a no-op, so it enters the history here.
        self.focus_pane_by_id(pane_id, ctx);
        self.update_pane_history(pane_id);
        ctx.notify();
        ctx.emit(super::Event::TerminalViewStateChanged);
        ctx.emit(super::Event::AppStateChanged);
        true
    }

    fn toggle_bottom_panel(&mut self, ctx: &mut ViewContext<Self>) {
        let dock = &self.warpium_bottom_dock.model;
        let (is_empty, is_open, active) = (dock.is_empty(), dock.is_open(), dock.active());
        if is_empty {
            self.open_terminal_in_bottom_panel(ctx);
        } else if is_open {
            self.hide_bottom_panel(ctx);
        } else if let Some(active) = active {
            self.warpium_bottom_dock.model.set_open(true);
            self.focus_pane_by_id(active, ctx);
            ctx.notify();
        }
    }

    fn hide_bottom_panel(&mut self, ctx: &mut ViewContext<Self>) {
        self.warpium_bottom_dock.model.set_open(false);
        if self.is_pane_docked(self.focused_pane_id(ctx)) {
            self.focus_after_leaving_dock(ctx);
        }
        ctx.notify();
    }

    /// Opens a new terminal as a sub-tab of the bottom panel and focuses it.
    fn open_terminal_in_bottom_panel(&mut self, ctx: &mut ViewContext<Self>) {
        let base_session_id = self.active_session_id(ctx);
        let initial_directory = self.startup_path_for_new_session(base_session_id, ctx);
        let ignore_custom_startup_directory =
            self.should_ignore_custom_startup_directory(&None, ctx);
        let startup_directory = SessionSettings::handle(ctx).read(ctx, |settings, _| {
            settings
                .working_directory_config
                .initial_directory_for_new_session(
                    NewSessionSource::SplitPane,
                    initial_directory,
                    ignore_custom_startup_directory,
                )
        });
        let (pane_data, _view) = self.create_terminal_pane_data(
            startup_directory,
            HashMap::new(),
            IsSharedSessionCreator::No,
            None, /* chosen_shell */
            None, /* conversation_restoration */
            ctx,
        );
        let Some(pane_id) = self.attach_child_pane_off_tree(Box::new(pane_data), ctx) else {
            log::warn!("[Bottom panel] New terminal pane could not be attached");
            return;
        };
        self.dock_pane(pane_id, None, ctx);
        self.focus_pane_by_id(pane_id, ctx);
        ctx.notify();
        ctx.emit(super::Event::AppStateChanged);
    }

    /// Splits a new terminal next to `pane_id`, the last visible pane of the layout.
    fn open_fresh_terminal_next_to(&mut self, pane_id: PaneId, ctx: &mut ViewContext<Self>) {
        self.add_session(
            Direction::Right,
            Some(pane_id),
            pane_id.as_terminal_pane_id(),
            None, /* chosen_shell */
            None, /* conversation_restoration */
            ctx,
        );
    }

    /// Records an off-tree pane as docked and shows it in the panel.
    fn dock_pane(
        &mut self,
        pane_id: PaneId,
        hint: Option<RestoreHint<PaneId>>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.warpium_bottom_dock.model.dock(pane_id, hint);
        self.warpium_bottom_dock
            .tab_mouse_states
            .entry(pane_id)
            .or_default();
        self.focus_state.update(ctx, |focus_state, _| {
            focus_state.set_pane_docked(pane_id, true);
        });
        if self.warpium_bottom_dock.title_subscriptions.insert(pane_id)
            && let Some(pane) = self.pane_contents.get(&pane_id)
        {
            let configuration = pane.as_pane().pane_configuration();
            ctx.subscribe_to_model(&configuration, move |me, _, event, ctx| {
                if matches!(event, PaneConfigurationEvent::TitleUpdated)
                    && me.is_pane_docked(pane_id)
                {
                    ctx.notify();
                }
            });
        }
    }

    /// Removes a pane from the dock (not from `pane_contents`) and returns its restore hint, or
    /// `None` if it wasn't docked.
    fn undock_pane(
        &mut self,
        pane_id: PaneId,
        ctx: &mut ViewContext<Self>,
    ) -> Option<Option<RestoreHint<PaneId>>> {
        let docked = self.warpium_bottom_dock.model.undock(pane_id)?;
        self.warpium_bottom_dock.tab_mouse_states.remove(&pane_id);
        self.focus_state.update(ctx, |focus_state, _| {
            focus_state.set_pane_docked(pane_id, false);
        });
        ctx.notify();
        Some(docked.restore_hint)
    }

    /// Moves focus off a docked pane that is leaving or being hidden: to the panel's newly
    /// selected sub-tab while the panel stays open, else to the layout.
    fn focus_after_leaving_dock(&mut self, ctx: &mut ViewContext<Self>) {
        let focused = self.focused_pane_id(ctx);
        let target = self
            .warpium_bottom_dock
            .model
            .visible_pane()
            .filter(|id| *id != focused)
            .or_else(|| self.warpium_layout_anchor(None));
        match target {
            Some(target) => self.focus_pane_by_id(target, ctx),
            None => log::warn!("[Bottom panel] No pane to focus after leaving the panel"),
        }
    }

    pub(super) fn render_warpium_bottom_dock(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        let active = self.warpium_bottom_dock.model.visible_pane()?;
        if !self.pane_contents.contains_key(&active) {
            return None;
        }
        // A maximized docked pane is rendered in place of the layout instead.
        if self.is_focused_pane_maximized(app) && self.focused_pane_id(app) == active {
            return None;
        }
        let appearance = Appearance::as_ref(app);
        let content = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.render_warpium_dock_tab_strip(active, appearance, app))
            .with_child(Shrinkable::new(1., active.render(app)).finish())
            .finish();
        let content = Container::new(content)
            .with_border(Border::top(1.).with_border_fill(appearance.theme().outline()))
            .finish();
        Some(
            Resizable::new(self.warpium_bottom_dock.height.clone(), content)
                .with_dragbar_side(DragBarSide::Top)
                .on_resize(|ctx, _| ctx.notify())
                .with_bounds_callback(Box::new(|window_size| {
                    let max = (window_size.y() - MIN_LAYOUT_HEIGHT).max(MIN_HEIGHT);
                    (MIN_HEIGHT, max)
                }))
                .finish(),
        )
    }

    fn render_warpium_dock_tab_strip(
        &self,
        active: PaneId,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let dock = &self.warpium_bottom_dock;
        let mut tabs = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        for pane_id in dock.model.ids() {
            let Some(mouse_states) = dock.tab_mouse_states.get(&pane_id) else {
                continue;
            };
            tabs.add_child(self.render_warpium_dock_tab(
                pane_id,
                pane_id == active,
                mouse_states,
                appearance,
                app,
            ));
        }
        let row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Shrinkable::new(1., Clipped::new(tabs.finish()).finish()).finish())
            .with_child(dock_button(
                Icon::ArrowUp,
                "Restore pane to layout",
                dock.restore_button.clone(),
                BottomPanelAction::Restore(active),
                appearance,
            ))
            .with_child(dock_button(
                Icon::ChevronDown,
                "Hide bottom panel",
                dock.hide_button.clone(),
                BottomPanelAction::Hide,
                appearance,
            ))
            .finish();
        Container::new(
            ConstrainedBox::new(row)
                .with_height(TAB_STRIP_HEIGHT)
                .finish(),
        )
        .with_horizontal_padding(4.)
        .with_border(Border::bottom(1.).with_border_fill(appearance.theme().outline()))
        .finish()
    }

    fn render_warpium_dock_tab(
        &self,
        pane_id: PaneId,
        is_active: bool,
        mouse_states: &TabMouseStates,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let title = self
            .pane_contents
            .get(&pane_id)
            .map(|pane| {
                pane.as_pane()
                    .pane_configuration()
                    .as_ref(app)
                    .title()
                    .to_owned()
            })
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "Terminal".to_owned());
        let text_color = if is_active {
            theme.active_ui_text_color()
        } else {
            theme.nonactive_ui_text_color()
        };
        let label = ConstrainedBox::new(
            Text::new_inline(title, appearance.ui_font_family(), TAB_FONT_SIZE)
                .with_color(text_color.into_solid())
                .with_clip(ClipConfig::end())
                .finish(),
        )
        .with_max_width(TAB_MAX_WIDTH)
        .finish();
        let label = Hoverable::new(mouse_states.tab.clone(), |_| {
            Container::new(label).with_horizontal_padding(6.).finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(PaneGroupAction::WarpiumBottomPanel(
                BottomPanelAction::Select(pane_id),
            ))
        })
        .with_cursor(Cursor::PointingHand)
        .finish();
        let close = icon_button(appearance, Icon::X, false, mouse_states.close.clone())
            .build()
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(PaneGroupAction::Remove(pane_id)))
            .with_cursor(Cursor::PointingHand)
            .finish();
        let tab = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(label)
            .with_child(close)
            .finish();
        let mut container =
            Container::new(tab).with_border(Border::bottom(2.).with_border_fill(if is_active {
                theme.accent()
            } else {
                theme.outline()
            }));
        if is_active {
            container = container.with_background(theme.surface_2());
        }
        container.finish()
    }
}

fn dock_button(
    icon: Icon,
    tooltip: &'static str,
    mouse_state: MouseStateHandle,
    action: BottomPanelAction,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let ui_builder = appearance.ui_builder().clone();
    icon_button(appearance, icon, false, mouse_state)
        .with_tooltip(move || ui_builder.tool_tip(tooltip.to_owned()).build().finish())
        .build()
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(PaneGroupAction::WarpiumBottomPanel(action.clone()))
        })
        .with_cursor(Cursor::PointingHand)
        .finish()
}

/// The visible part of the layout tree, for finding a pane's neighbors.
fn layout_of(node: &PaneNode, panes: &PaneData) -> Option<LayoutNode<PaneId>> {
    match node {
        PaneNode::Leaf(pane_id) => {
            (!panes.is_pane_hidden(pane_id)).then_some(LayoutNode::Leaf(*pane_id))
        }
        PaneNode::Branch(branch) => {
            let children: Vec<_> = branch
                .nodes
                .iter()
                .filter_map(|(_, child)| layout_of(child, panes))
                .collect();
            let axis = match branch.axis() {
                SplitDirection::Horizontal => Axis::Horizontal,
                SplitDirection::Vertical => Axis::Vertical,
            };
            (!children.is_empty()).then_some(LayoutNode::Split { axis, children })
        }
    }
}

fn direction_of(side: Side) -> Direction {
    match side {
        Side::Left => Direction::Left,
        Side::Right => Direction::Right,
        Side::Up => Direction::Up,
        Side::Down => Direction::Down,
    }
}
