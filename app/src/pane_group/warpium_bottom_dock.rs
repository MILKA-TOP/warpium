//! Warpium: the bottom panel, a per-tab dock below the split layout (roadmap item 14).
//!
//! A docked pane leaves the layout tree but stays in `pane_contents`, so its view, terminal
//! manager and PTY keep running, like a child agent pane that is off-tree. The dock records which
//! panes are docked, in which order, which one it shows and whether it is expanded or collapsed to
//! a strip of status chips (the model is `warpium_bottom_panel::BottomDock`). Everything that walks
//! the layout tree (focus navigation, splits, resizing, drag and drop) is kept away from docked
//! panes here, and every path that would empty the layout (closing or moving away its last pane)
//! first opens a fresh terminal so the tab, and the processes in its dock, survive.
//!
//! Rendering is in `warpium_bottom_dock_render.rs`, the saved form in
//! `warpium_bottom_dock_persistence.rs`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::PathBuf;

use warpium_bottom_panel::{
    Axis, BottomDock, DockOrigin, DockedPane, LayoutNode, RestoreHint, Side,
    fresh_terminal_directory, layout_anchor, restore_hint,
};
use warpui::r#async::Timer;
use warpui::elements::{
    DraggableState, MouseStateHandle, ResizableStateHandle, resizable_state_handle,
};
use warpui::keymap::{Context, EditableBinding};
use warpui::{AppContext, SingletonEntity as _, ViewContext, ViewHandle};

use super::pane::view::PaneViewEvent;
use super::{
    DefaultSessionModeBehavior, DetachType, Direction, PaneConfigurationEvent, PaneData, PaneGroup,
    PaneGroupAction, PaneId, PaneNode, SplitDirection,
};
use crate::menu::{Event as MenuEvent, Menu, MenuItem, MenuItemFields, SubMenu};
use crate::pane_group::AnyPaneContent;
use crate::terminal::TerminalView;
use crate::terminal::session_settings::{NewSessionSource, SessionSettings};
use crate::terminal::shared_session::IsSharedSessionCreator;
use crate::warpium::sync_settings::WarpiumBottomPanelSettings;
use crate::workspace::WorkspaceRegistry;

#[path = "warpium_bottom_dock_persistence.rs"]
mod persistence;
#[path = "warpium_bottom_dock_render.rs"]
mod render;

/// Keymap context set while the focused pane is docked.
const FOCUSED_PANE_DOCKED: &str = "PaneGroup_FocusedPaneDocked";
/// Keymap context set while the focused pane can move to the bottom panel.
const FOCUSED_PANE_DOCKABLE: &str = "PaneGroup_FocusedPaneDockable";

const DEFAULT_HEIGHT: f32 = 260.;
const MENU_WIDTH: f32 = 240.;
/// How often hidden sub-tabs are checked for new output and status changes.
const STATUS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Moves a pane between the layout and the bottom panel, from a pane's own menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomPanelPaneRequest {
    MoveToBottomPanel,
    RestoreToLayout,
}

/// Where a docked sub-tab moves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomPanelTabTarget {
    /// The bottom panel of the tab at this index.
    Tab(usize),
    /// A new tab, as its only pane.
    NewTab,
}

/// Where a dragged sub-tab chip was dropped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BottomPanelChipDrop {
    /// On another chip: reorder.
    Chip(PaneId),
    /// On a pane of the layout, at this point (window coordinates): restore next to it.
    LayoutPane { target: PaneId, x: f32, y: f32 },
    /// On the tab bar.
    TabBar(BottomPanelTabTarget),
}

#[derive(Debug, Clone)]
pub enum BottomPanelAction {
    /// Expands or collapses the panel; opens a new terminal in it when it is empty.
    Toggle,
    /// Collapses the panel to its strip.
    Collapse,
    /// Expands the panel on its selected sub-tab.
    Expand,
    MoveFocusedPane,
    RestoreFocusedPane,
    /// Shows and focuses the sub-tab of a docked pane.
    Select(PaneId),
    Restore(PaneId),
    Close(PaneId),
    NewTerminal,
    SelectNext,
    SelectPrevious,
    ToggleMaximized,
    /// Opens a sub-tab's context menu.
    OpenSubTabMenu(PaneId),
    MoveToTab {
        pane_id: PaneId,
        target: BottomPanelTabTarget,
    },
    ChipDropped {
        pane_id: PaneId,
        drop: BottomPanelChipDrop,
    },
}

/// Requests the bottom panel makes of the workspace.
#[derive(Debug, Clone)]
pub enum BottomPanelEvent {
    /// Move a docked pane to another tab's panel, or to a new tab.
    MoveToTab {
        pane_id: PaneId,
        target: BottomPanelTabTarget,
    },
}

/// A docking or restore picked while dragging a pane header, applied when it is dropped.
#[derive(Debug, Clone, Copy)]
enum PendingDrop {
    Dock(PaneId),
    Restore {
        pane_id: PaneId,
        target: PaneId,
        direction: Direction,
    },
}

/// What a chip shows, compared between polls to re-render only on change.
type ChipFingerprint = (PaneId, warpium_bottom_panel::SubTabStatus, bool);

/// The bottom panel of one tab.
pub(super) struct WarpiumBottomDock {
    model: BottomDock<PaneId>,
    height: ResizableStateHandle,
    chip_states: HashMap<PaneId, ChipStates>,
    buttons: DockButtons,
    /// Docked panes whose title changes re-render the chips.
    title_subscriptions: HashSet<PaneId>,
    /// The sub-tab context menu, created the first time it opens.
    menu: Option<ViewHandle<Menu<PaneGroupAction>>>,
    /// The sub-tab whose context menu is open.
    menu_pane: Option<PaneId>,
    pending_drop: Option<PendingDrop>,
    chip_fingerprints: Vec<ChipFingerprint>,
    poll_scheduled: bool,
    settings_subscribed: bool,
}

#[derive(Default)]
struct ChipStates {
    chip: MouseStateHandle,
    close: MouseStateHandle,
    drag: DraggableState,
}

#[derive(Default)]
struct DockButtons {
    expand: MouseStateHandle,
    collapse: MouseStateHandle,
    maximize: MouseStateHandle,
    restore: MouseStateHandle,
    new_terminal: MouseStateHandle,
}

impl Default for WarpiumBottomDock {
    fn default() -> Self {
        Self {
            model: BottomDock::default(),
            height: resizable_state_handle(DEFAULT_HEIGHT),
            chip_states: HashMap::new(),
            buttons: DockButtons::default(),
            title_subscriptions: HashSet::new(),
            menu: None,
            menu_pane: None,
            pending_drop: None,
            chip_fingerprints: Vec::new(),
            poll_scheduled: false,
            settings_subscribed: false,
        }
    }
}

pub(super) fn init_bindings(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_editable_bindings([
        EditableBinding::new(
            "pane_group:move_pane_to_bottom_panel",
            "Move pane to bottom panel",
            PaneGroupAction::WarpiumBottomPanel(BottomPanelAction::MoveFocusedPane),
        )
        .with_context_predicate(id!("PaneGroup") & id!(FOCUSED_PANE_DOCKABLE)),
        EditableBinding::new(
            "pane_group:restore_pane_from_bottom_panel",
            "Restore pane to layout",
            PaneGroupAction::WarpiumBottomPanel(BottomPanelAction::RestoreFocusedPane),
        )
        .with_context_predicate(id!("PaneGroup") & id!(FOCUSED_PANE_DOCKED)),
    ]);
}

fn is_enabled(app: &AppContext) -> bool {
    crate::warpium::bottom_panel::is_enabled(app)
}

impl PaneGroup {
    pub(crate) fn is_pane_docked(&self, pane_id: PaneId) -> bool {
        self.warpium_bottom_dock.model.contains(pane_id)
    }

    #[cfg(test)]
    pub(super) fn warpium_bottom_dock_model(&self) -> &BottomDock<PaneId> {
        &self.warpium_bottom_dock.model
    }

    /// Whether the panel is expanded and maximized, taking the layout's place.
    pub(super) fn is_warpium_bottom_panel_maximized(&self) -> bool {
        self.warpium_bottom_dock.model.is_maximized()
    }

    fn is_logcat_pane(&self, pane_id: PaneId) -> bool {
        #[cfg(not(target_family = "wasm"))]
        {
            self.downcast_pane_by_id::<super::LogcatPane>(pane_id)
                .is_some()
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = pane_id;
            false
        }
    }

    /// Whether the pane can move to the bottom panel: a visible terminal or Logcat pane of the
    /// layout that no other mechanism (child agents, temporary swaps) manages.
    fn is_pane_dockable(&self, pane_id: PaneId, app: &AppContext) -> bool {
        is_enabled(app)
            && (pane_id.as_terminal_pane_id().is_some() || self.is_logcat_pane(pane_id))
            && self.pane_contents.contains_key(&pane_id)
            && self.panes.is_pane_in_tree(pane_id)
            && !self.panes.is_pane_hidden(&pane_id)
            && !self.is_child_agent_pane(pane_id)
            && !self.panes.is_temporary_replacement(pane_id)
            && self.panes.replacement_pane_for_original(pane_id).is_none()
    }

    /// The layout pane to act on instead of a docked one: the most recently focused visible pane
    /// of the layout.
    fn warpium_layout_anchor(&self) -> Option<PaneId> {
        layout_anchor(&self.pane_history, &self.panes.visible_pane_ids())
    }

    fn output_generation(&self, pane_id: PaneId, app: &AppContext) -> u64 {
        self.terminal_view_from_pane_id(pane_id, app)
            .map_or(0, |view| view.as_ref(app).warpium_output_generation())
    }

    /// Applies `change` to the dock model. When it changes the visible sub-tab, the output of the
    /// one that is no longer visible counts as seen up to now.
    fn update_dock<R>(
        &mut self,
        app: &AppContext,
        change: impl FnOnce(&mut BottomDock<PaneId>) -> R,
    ) -> R {
        let before = self.warpium_bottom_dock.model.visible_pane();
        let result = change(&mut self.warpium_bottom_dock.model);
        if let Some(hidden) = before
            && self.warpium_bottom_dock.model.visible_pane() != before
        {
            let generation = self.output_generation(hidden, app);
            self.warpium_bottom_dock.model.mark_seen(hidden, generation);
        }
        result
    }

    /// Hook for adding a pane next to `base_pane_id`: a docked base is replaced by the layout
    /// pane the user last focused, since docked panes aren't in the tree to split.
    pub(super) fn warpium_split_base(&self, base_pane_id: PaneId) -> PaneId {
        if !self.is_pane_docked(base_pane_id) {
            return base_pane_id;
        }
        self.warpium_layout_anchor().unwrap_or(base_pane_id)
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
        self.reassign_active_session_from(pane_id, ctx);
        self.remove_from_pane_history(pane_id);
        self.unsubscribe_from_docked_title(pane_id, ctx);
        // Dropping the pane's contents drops its terminal manager, which ends the PTY.
        self.discard_pane(pane_id, ctx);
        self.handle_pane_count_change(ctx);
        ctx.emit(super::Event::TerminalViewStateChanged);
        ctx.emit(super::Event::AppStateChanged);
        true
    }

    fn reassign_active_session_from(&mut self, pane_id: PaneId, ctx: &mut ViewContext<Self>) {
        if Some(pane_id) == self.active_session_id(ctx).map(Into::into) {
            let new_active_session = self.choose_active_session(pane_id);
            self.focus_state.update(ctx, |focus_state, ctx| {
                focus_state.set_active_session(new_active_session, ctx);
            });
        }
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
            // Docked panes leave through `take_docked_pane_for_move`; keep the dock consistent if
            // one reaches the layout's paths anyway.
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
    pub(super) fn warpium_on_pane_focused(&mut self, pane_id: PaneId, app: &AppContext) {
        if self.is_pane_docked(pane_id) {
            self.update_dock(app, |dock| dock.select(pane_id));
        }
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

    /// Hook for pane view events (pane header drags). A layout pane dragged onto the panel is
    /// docked, a docked pane dragged onto the layout is restored there, both once dropped; docked
    /// panes can't be dragged onto the tab bar. Returns true if it handled `event`.
    pub(super) fn warpium_handle_pane_view_event(
        &mut self,
        pane_id: PaneId,
        event: &PaneViewEvent,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        match event {
            PaneViewEvent::MovePaneWithinPaneGroup {
                target_id,
                direction,
            } => {
                let source_docked = self.is_pane_docked(pane_id);
                let target_docked = self.is_pane_docked(*target_id);
                if !source_docked && !target_docked {
                    self.warpium_bottom_dock.pending_drop = None;
                    return false;
                }
                self.warpium_bottom_dock.pending_drop = match (source_docked, target_docked) {
                    // Only a pane whose layout keeps another pane: docking the last one would
                    // open a fresh terminal mid-drag.
                    (false, true)
                        if self.is_pane_dockable(pane_id, ctx)
                            && self.panes.visible_pane_count() > 1 =>
                    {
                        Some(PendingDrop::Dock(pane_id))
                    }
                    (true, false) => Some(PendingDrop::Restore {
                        pane_id,
                        target: *target_id,
                        direction: *direction,
                    }),
                    _ => None,
                };
                true
            }
            PaneViewEvent::DroppedOnTabBar { .. } | PaneViewEvent::DraggedOntoTabBar { .. } => {
                self.warpium_bottom_dock.pending_drop = None;
                self.is_pane_docked(pane_id)
            }
            PaneViewEvent::PaneDraggedOutsideTabBarOrPaneGroup => {
                self.warpium_bottom_dock.pending_drop = None;
                false
            }
            PaneViewEvent::PaneDragEnded => {
                match self.warpium_bottom_dock.pending_drop.take() {
                    Some(PendingDrop::Dock(dragged)) if dragged == pane_id => {
                        self.move_pane_to_bottom_panel(dragged, ctx);
                    }
                    Some(PendingDrop::Restore {
                        pane_id: dragged,
                        target,
                        direction,
                    }) if dragged == pane_id => {
                        self.restore_docked_pane_at(dragged, target, direction, ctx);
                    }
                    _ => {}
                }
                false
            }
            PaneViewEvent::PaneHeaderClicked => false,
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
                self.focus_after_leaving_dock_to_layout(ctx);
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
        if !is_enabled(app) {
            return;
        }
        let focused = self.focused_pane_id(app);
        if self.is_pane_docked(focused) {
            context.set.insert(FOCUSED_PANE_DOCKED);
        } else if self.is_pane_dockable(focused, app) {
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

    pub(crate) fn handle_warpium_bottom_panel_action(
        &mut self,
        action: &BottomPanelAction,
        ctx: &mut ViewContext<Self>,
    ) {
        // Getting panes back out keeps working after the setting is turned off.
        let leaves_the_panel = matches!(
            action,
            BottomPanelAction::Restore(_)
                | BottomPanelAction::RestoreFocusedPane
                | BottomPanelAction::Close(_)
        );
        if !leaves_the_panel && !is_enabled(ctx) {
            return;
        }
        match action {
            BottomPanelAction::Toggle => self.toggle_bottom_panel(ctx),
            BottomPanelAction::Collapse => self.collapse_bottom_panel(ctx),
            BottomPanelAction::Expand => {
                self.update_dock(ctx, |dock| dock.set_expanded(true));
                if let Some(active) = self.warpium_bottom_dock.model.active() {
                    self.focus_pane_by_id(active, ctx);
                }
                ctx.notify();
                ctx.emit(super::Event::AppStateChanged);
            }
            BottomPanelAction::MoveFocusedPane => {
                let pane_id = self.focused_pane_id(ctx);
                self.move_pane_to_bottom_panel(pane_id, ctx);
            }
            BottomPanelAction::RestoreFocusedPane => {
                let pane_id = self.focused_pane_id(ctx);
                self.restore_pane_from_bottom_panel(pane_id, ctx);
            }
            BottomPanelAction::Select(pane_id) => {
                // Focusing an already focused pane is a no-op, so select explicitly too.
                self.update_dock(ctx, |dock| dock.select(*pane_id));
                self.focus_pane_by_id(*pane_id, ctx);
                ctx.notify();
            }
            BottomPanelAction::Restore(pane_id) => {
                self.restore_pane_from_bottom_panel(*pane_id, ctx);
            }
            BottomPanelAction::Close(pane_id) => {
                if self.is_pane_docked(*pane_id) {
                    self.close_pane(*pane_id, ctx);
                }
            }
            BottomPanelAction::NewTerminal => self.open_terminal_in_bottom_panel(ctx),
            BottomPanelAction::SelectNext | BottomPanelAction::SelectPrevious => {
                let next = matches!(action, BottomPanelAction::SelectNext);
                let selected = self.update_dock(ctx, |dock| {
                    if next {
                        dock.select_next()
                    } else {
                        dock.select_previous()
                    }
                });
                if let Some(selected) = selected {
                    self.focus_pane_by_id(selected, ctx);
                }
                ctx.notify();
            }
            BottomPanelAction::ToggleMaximized => {
                let Some(active) = self.warpium_bottom_dock.model.active() else {
                    return;
                };
                let maximized = self.update_dock(ctx, |dock| dock.toggle_maximized());
                if maximized {
                    self.focus_pane_by_id(active, ctx);
                }
                ctx.notify();
                ctx.emit(super::Event::AppStateChanged);
            }
            BottomPanelAction::OpenSubTabMenu(pane_id) => self.open_sub_tab_menu(*pane_id, ctx),
            BottomPanelAction::MoveToTab { pane_id, target } => {
                self.close_sub_tab_menu(ctx);
                if self.is_pane_docked(*pane_id) {
                    ctx.emit(super::Event::WarpiumBottomPanel(
                        BottomPanelEvent::MoveToTab {
                            pane_id: *pane_id,
                            target: *target,
                        },
                    ));
                }
            }
            BottomPanelAction::ChipDropped { pane_id, drop } => {
                self.handle_chip_drop(*pane_id, *drop, ctx)
            }
        }
    }

    fn handle_chip_drop(
        &mut self,
        pane_id: PaneId,
        drop: BottomPanelChipDrop,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.is_pane_docked(pane_id) {
            return;
        }
        match drop {
            BottomPanelChipDrop::Chip(target) => {
                let dock = &self.warpium_bottom_dock.model;
                let after = dock.index_of(pane_id) < dock.index_of(target);
                if self
                    .warpium_bottom_dock
                    .model
                    .move_entry(pane_id, target, after)
                {
                    ctx.notify();
                    ctx.emit(super::Event::AppStateChanged);
                }
            }
            BottomPanelChipDrop::LayoutPane { target, x, y } => {
                if self.is_pane_docked(target) || !self.panes.is_pane_in_tree(target) {
                    return;
                }
                let side = ctx
                    .element_position_by_id(target.position_id())
                    .map(|rect| {
                        warpium_bottom_panel::drop_side(
                            (
                                rect.origin_x(),
                                rect.origin_y(),
                                rect.width(),
                                rect.height(),
                            ),
                            (x, y),
                        )
                    })
                    .unwrap_or(Side::Right);
                self.restore_docked_pane_at(pane_id, target, direction_of(side), ctx);
            }
            BottomPanelChipDrop::TabBar(target) => {
                ctx.emit(super::Event::WarpiumBottomPanel(
                    BottomPanelEvent::MoveToTab { pane_id, target },
                ));
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
        if !self.is_pane_dockable(pane_id, ctx) {
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
        let origin = if self.is_logcat_pane(pane_id) {
            DockOrigin::Logcat
        } else {
            DockOrigin::Manual
        };
        let generation = self.output_generation(pane_id, ctx);
        self.dock_pane(
            DockedPane::new(pane_id, origin, generation).with_restore_hint(hint),
            ctx,
        );
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
                self.warpium_layout_anchor()
                    .map(|anchor| (anchor, Direction::Right))
            });
        self.insert_restored_pane(pane_id, target, ctx);
        true
    }

    /// Puts a docked pane into the layout next to `target`, towards `direction`.
    fn restore_docked_pane_at(
        &mut self,
        pane_id: PaneId,
        target: PaneId,
        direction: Direction,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.undock_pane(pane_id, ctx).is_none() {
            return;
        }
        self.insert_restored_pane(pane_id, Some((target, direction)), ctx);
    }

    fn insert_restored_pane(
        &mut self,
        pane_id: PaneId,
        target: Option<(PaneId, Direction)>,
        ctx: &mut ViewContext<Self>,
    ) {
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
    }

    /// Opens the context menu of a sub-tab: restore, move to another tab, close.
    fn open_sub_tab_menu(&mut self, pane_id: PaneId, ctx: &mut ViewContext<Self>) {
        if !self.is_pane_docked(pane_id) {
            return;
        }
        let menu = match self.warpium_bottom_dock.menu.clone() {
            Some(menu) => menu,
            None => {
                let menu = ctx.add_typed_action_view(|_| {
                    Menu::new()
                        .with_width(MENU_WIDTH)
                        .with_drop_shadow()
                        .prevent_interaction_with_other_elements()
                });
                ctx.subscribe_to_view(&menu, |me, _, event, ctx| match event {
                    MenuEvent::Close { .. } => me.close_sub_tab_menu(ctx),
                    MenuEvent::ItemSelected | MenuEvent::ItemHovered => {}
                });
                self.warpium_bottom_dock.menu = Some(menu.clone());
                menu
            }
        };
        let action = |action| PaneGroupAction::WarpiumBottomPanel(action);
        let move_to = |target| action(BottomPanelAction::MoveToTab { pane_id, target });
        let mut tab_items: Vec<MenuItem<PaneGroupAction>> = self
            .other_tab_titles(ctx)
            .into_iter()
            .map(|(index, title)| {
                MenuItemFields::new(format!("Tab {}: {title}", index + 1))
                    .with_on_select_action(move_to(BottomPanelTabTarget::Tab(index)))
                    .into_item()
            })
            .collect();
        tab_items.push(
            MenuItemFields::new("New tab")
                .with_on_select_action(move_to(BottomPanelTabTarget::NewTab))
                .into_item(),
        );
        let items = vec![
            MenuItemFields::new("Restore pane to layout")
                .with_on_select_action(action(BottomPanelAction::Restore(pane_id)))
                .into_item(),
            MenuItem::Submenu {
                fields: MenuItemFields::new_submenu("Move to tab"),
                menu: SubMenu::new(tab_items),
            },
            MenuItem::Separator,
            MenuItemFields::new("Close")
                .with_on_select_action(action(BottomPanelAction::Close(pane_id)))
                .into_item(),
        ];
        menu.update(ctx, |menu, ctx| menu.set_items(items, ctx));
        self.warpium_bottom_dock.menu_pane = Some(pane_id);
        ctx.focus(&menu);
        ctx.notify();
    }

    fn close_sub_tab_menu(&mut self, ctx: &mut ViewContext<Self>) {
        if self.warpium_bottom_dock.menu_pane.take().is_some() {
            ctx.notify();
        }
    }

    /// The window's other tabs, by index, with their titles.
    fn other_tab_titles(&self, ctx: &ViewContext<Self>) -> Vec<(usize, String)> {
        if !ctx.has_singleton_model::<WorkspaceRegistry>() {
            return Vec::new();
        }
        let Some(workspace) = WorkspaceRegistry::as_ref(ctx).get(ctx.window_id(), ctx) else {
            return Vec::new();
        };
        let own_id = ctx.view_id();
        workspace
            .as_ref(ctx)
            .tab_views()
            .enumerate()
            .filter(|(_, pane_group)| pane_group.id() != own_id)
            .map(|(index, pane_group)| (index, pane_group.as_ref(ctx).display_title(ctx)))
            .collect()
    }

    /// Restores every docked pane to the layout (the setting was turned off).
    fn restore_all_docked_panes(&mut self, ctx: &mut ViewContext<Self>) {
        let docked: Vec<PaneId> = self.warpium_bottom_dock.model.ids().collect();
        for pane_id in docked {
            self.restore_pane_from_bottom_panel(pane_id, ctx);
        }
    }

    fn toggle_bottom_panel(&mut self, ctx: &mut ViewContext<Self>) {
        let dock = &self.warpium_bottom_dock.model;
        let (is_empty, is_expanded, active) = (dock.is_empty(), dock.is_expanded(), dock.active());
        if is_empty {
            self.open_terminal_in_bottom_panel(ctx);
        } else if is_expanded {
            self.collapse_bottom_panel(ctx);
        } else if let Some(active) = active {
            self.update_dock(ctx, |dock| dock.set_expanded(true));
            self.focus_pane_by_id(active, ctx);
            ctx.notify();
            ctx.emit(super::Event::AppStateChanged);
        }
    }

    fn collapse_bottom_panel(&mut self, ctx: &mut ViewContext<Self>) {
        self.update_dock(ctx, |dock| dock.set_expanded(false));
        if self.is_pane_docked(self.focused_pane_id(ctx)) {
            self.focus_after_leaving_dock_to_layout(ctx);
        }
        ctx.notify();
        ctx.emit(super::Event::AppStateChanged);
    }

    /// The working directory for a terminal opened from the panel: the tab's locked project
    /// root, else the active session's directory, else the new-session setting's default.
    fn bottom_panel_terminal_directory(&self, ctx: &mut ViewContext<Self>) -> Option<PathBuf> {
        if let Some(root) = self.locked_project_root() {
            return Some(root);
        }
        let base_session_id = self.active_session_id(ctx);
        let initial_directory = self.startup_path_for_new_session(base_session_id, ctx);
        let ignore_custom_startup_directory =
            self.should_ignore_custom_startup_directory(&None, ctx);
        SessionSettings::handle(ctx).read(ctx, |settings, _| {
            settings
                .working_directory_config
                .initial_directory_for_new_session(
                    NewSessionSource::SplitPane,
                    initial_directory,
                    ignore_custom_startup_directory,
                )
        })
    }

    fn locked_project_root(&self) -> Option<PathBuf> {
        self.project_root()
            .primary()
            .filter(|root| root.is_dir())
            .map(|root| root.to_path_buf())
    }

    /// Opens a new terminal as a sub-tab of the bottom panel and focuses it.
    fn open_terminal_in_bottom_panel(&mut self, ctx: &mut ViewContext<Self>) {
        let directory = self.bottom_panel_terminal_directory(ctx);
        if let Some(pane_id) = self.add_terminal_to_bottom_panel(
            directory,
            HashMap::new(),
            DockOrigin::Manual,
            None,
            ctx,
        ) {
            self.focus_pane_by_id(pane_id, ctx);
        }
    }

    /// Starts a terminal in `cwd` with `env` added to its environment as a new sub-tab, selected
    /// (expanding the panel) but not focused.
    fn add_terminal_to_bottom_panel(
        &mut self,
        cwd: Option<PathBuf>,
        env: HashMap<OsString, OsString>,
        origin: DockOrigin,
        title: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) -> Option<PaneId> {
        let (pane_data, _view) = self.create_terminal_pane_data(
            cwd,
            env,
            IsSharedSessionCreator::No,
            None, /* chosen_shell */
            None, /* conversation_restoration */
            ctx,
        );
        self.adopt_pane_into_bottom_panel(Box::new(pane_data), origin, title, ctx)
    }

    /// Adds a pane that isn't in this tab yet (new, or moved from another tab) as a sub-tab,
    /// selected and expanded but not focused.
    pub(crate) fn adopt_pane_into_bottom_panel(
        &mut self,
        pane: Box<dyn AnyPaneContent>,
        origin: DockOrigin,
        title: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) -> Option<PaneId> {
        let Some(pane_id) = self.attach_child_pane_off_tree(pane, ctx) else {
            log::warn!("[Bottom panel] Pane could not be attached to the bottom panel");
            return None;
        };
        let generation = self.output_generation(pane_id, ctx);
        self.dock_pane(
            DockedPane::new(pane_id, origin, generation).with_title(title),
            ctx,
        );
        ctx.notify();
        ctx.emit(super::Event::TerminalViewStateChanged);
        ctx.emit(super::Event::AppStateChanged);
        Some(pane_id)
    }

    /// Takes a docked pane out of this tab to move it elsewhere, with its sub-tab (origin and
    /// title). Unlike `remove_pane_for_move`, it never touches the layout, so the tab survives.
    pub(crate) fn take_docked_pane_for_move(
        &mut self,
        pane_id: PaneId,
        ctx: &mut ViewContext<Self>,
    ) -> Option<(Box<dyn AnyPaneContent>, DockedPane<PaneId>)> {
        if !self.is_pane_docked(pane_id) {
            return None;
        }
        let was_focused = self.focused_pane_id(ctx) == pane_id;
        let entry = self.warpium_bottom_dock.model.entry(pane_id).cloned()?;
        self.undock_pane(pane_id, ctx);
        if was_focused {
            self.focus_after_leaving_dock(ctx);
        }
        self.reassign_active_session_from(pane_id, ctx);
        if let Some(pane) = self.pane_contents.get(&pane_id) {
            pane.as_pane().detach(self, DetachType::Moved, ctx);
        }
        self.unsubscribe_from_docked_title(pane_id, ctx);
        let pane = self.pane_contents.remove(&pane_id)?;
        self.handle_pane_count_change(ctx);
        ctx.notify();
        ctx.emit(super::Event::TerminalViewStateChanged);
        ctx.emit(super::Event::AppStateChanged);
        Some((pane, entry))
    }

    /// Whether `pane_id` can leave this tab without closing it: it is docked, or the layout keeps
    /// another pane (or a fresh one, for a tab with docked panes).
    pub(crate) fn can_move_pane_out_keeping_tab(&self, pane_id: PaneId) -> bool {
        self.is_pane_docked(pane_id)
            || !self.warpium_bottom_dock.model.is_empty()
            || self.panes.visible_pane_count() > 1
    }

    /// The idle terminal of a sub-tab started by `key` (a toolbar button, an Android run), for
    /// that requester's next run. It is selected, expanding the panel, without taking focus.
    pub(crate) fn idle_bottom_panel_session(
        &mut self,
        key: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Option<ViewHandle<TerminalView>> {
        let origin = DockOrigin::Command {
            key: key.to_owned(),
        };
        let pane_id =
            self.warpium_bottom_dock
                .model
                .idle_entry_with_origin(&origin, |pane_id| {
                    self.terminal_view_from_pane_id(pane_id, ctx)
                        .is_none_or(|view| view.as_ref(ctx).warpium_is_busy(ctx))
                })?;
        self.update_dock(ctx, |dock| dock.select(pane_id));
        ctx.notify();
        self.terminal_view_from_pane_id(pane_id, ctx)
    }

    /// Starts a terminal for `key`'s run in a new sub-tab titled after `label` ("label (2)" while
    /// its earlier sub-tabs are busy), selected without taking focus.
    pub(crate) fn add_bottom_panel_session_for_command(
        &mut self,
        key: &str,
        label: &str,
        cwd: PathBuf,
        env: HashMap<OsString, OsString>,
        ctx: &mut ViewContext<Self>,
    ) -> Option<ViewHandle<TerminalView>> {
        let origin = DockOrigin::Command {
            key: key.to_owned(),
        };
        let title = self
            .warpium_bottom_dock
            .model
            .title_for_new_entry(&origin, label);
        let pane_id =
            self.add_terminal_to_bottom_panel(Some(cwd), env, origin, Some(title), ctx)?;
        self.terminal_view_from_pane_id(pane_id, ctx)
    }

    /// Shows a docked pane on its sub-tab; `focus` also focuses it.
    pub(crate) fn reveal_docked_pane(
        &mut self,
        pane_id: PaneId,
        focus: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if focus {
            self.focus_pane_by_id(pane_id, ctx);
        } else {
            self.update_dock(ctx, |dock| dock.select(pane_id));
        }
        ctx.notify();
    }

    /// Splits a new terminal next to `pane_id`, the last visible pane of the layout, in the tab's
    /// locked project root or else `pane_id`'s working directory.
    fn open_fresh_terminal_next_to(&mut self, pane_id: PaneId, ctx: &mut ViewContext<Self>) {
        let pane_cwd = self
            .terminal_view_from_pane_id(pane_id, ctx)
            .and_then(|view| view.as_ref(ctx).active_session_path_if_local(ctx))
            .filter(|cwd| cwd.is_dir());
        match fresh_terminal_directory(self.locked_project_root(), pane_cwd) {
            Some(directory) => {
                self.add_session_in_directory(
                    Direction::Right,
                    Some(pane_id),
                    None, /* chosen_shell */
                    Some(directory),
                    None, /* conversation_restoration */
                    DefaultSessionModeBehavior::Apply,
                    ctx,
                );
            }
            None => {
                self.add_session(
                    Direction::Right,
                    Some(pane_id),
                    pane_id.as_terminal_pane_id(),
                    None, /* chosen_shell */
                    None, /* conversation_restoration */
                    ctx,
                );
            }
        }
    }

    /// Records an off-tree pane as docked and shows it in the panel.
    fn dock_pane(&mut self, docked: DockedPane<PaneId>, ctx: &mut ViewContext<Self>) {
        let pane_id = docked.id;
        self.update_dock(ctx, |dock| dock.dock(docked));
        self.register_docked_pane(pane_id, ctx);
    }

    /// Sets up what a docked pane needs beyond the model: chip mouse state, the focus state's
    /// docked set, title updates, the setting subscription and the chip status poll.
    fn register_docked_pane(&mut self, pane_id: PaneId, ctx: &mut ViewContext<Self>) {
        self.warpium_bottom_dock
            .chip_states
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
        self.subscribe_to_bottom_panel_setting(ctx);
        self.schedule_chip_status_poll(ctx);
    }

    fn unsubscribe_from_docked_title(&mut self, pane_id: PaneId, ctx: &mut ViewContext<Self>) {
        if self
            .warpium_bottom_dock
            .title_subscriptions
            .remove(&pane_id)
            && let Some(pane) = self.pane_contents.get(&pane_id)
        {
            ctx.unsubscribe_to_model(&pane.as_pane().pane_configuration());
        }
    }

    /// Turning the setting off puts every docked pane back into the layout, so none is stranded.
    fn subscribe_to_bottom_panel_setting(&mut self, ctx: &mut ViewContext<Self>) {
        if std::mem::replace(&mut self.warpium_bottom_dock.settings_subscribed, true) {
            return;
        }
        ctx.subscribe_to_model(&WarpiumBottomPanelSettings::handle(ctx), |me, _, _, ctx| {
            if !is_enabled(ctx) {
                me.restore_all_docked_panes(ctx);
            }
        });
    }

    /// Checks the chips once a second while anything is docked: output and command state change
    /// inside the terminals without telling the pane group.
    fn schedule_chip_status_poll(&mut self, ctx: &mut ViewContext<Self>) {
        if std::mem::replace(&mut self.warpium_bottom_dock.poll_scheduled, true) {
            return;
        }
        ctx.spawn(
            async move { Timer::after(STATUS_POLL_INTERVAL).await },
            |me, _, ctx| {
                me.warpium_bottom_dock.poll_scheduled = false;
                if me.warpium_bottom_dock.model.is_empty() {
                    me.warpium_bottom_dock.chip_fingerprints.clear();
                    return;
                }
                let fingerprints = me.chip_fingerprints(ctx);
                if fingerprints != me.warpium_bottom_dock.chip_fingerprints {
                    me.warpium_bottom_dock.chip_fingerprints = fingerprints;
                    ctx.notify();
                }
                me.schedule_chip_status_poll(ctx);
            },
        );
    }

    fn chip_fingerprints(&self, app: &AppContext) -> Vec<ChipFingerprint> {
        self.warpium_bottom_dock
            .model
            .ids()
            .map(|pane_id| {
                let (status, unread) = self.chip_status(pane_id, app);
                (pane_id, status, unread)
            })
            .collect()
    }

    /// A sub-tab's status and whether it has unread output.
    fn chip_status(
        &self,
        pane_id: PaneId,
        app: &AppContext,
    ) -> (warpium_bottom_panel::SubTabStatus, bool) {
        match self.terminal_view_from_pane_id(pane_id, app) {
            Some(view) => {
                let view = view.as_ref(app);
                let unread = self
                    .warpium_bottom_dock
                    .model
                    .has_unread(pane_id, view.warpium_output_generation());
                (view.warpium_sub_tab_status(), unread)
            }
            None => (warpium_bottom_panel::SubTabStatus::Idle, false),
        }
    }

    /// Removes a pane from the dock (not from `pane_contents`) and returns its restore hint, or
    /// `None` if it wasn't docked.
    fn undock_pane(
        &mut self,
        pane_id: PaneId,
        ctx: &mut ViewContext<Self>,
    ) -> Option<Option<RestoreHint<PaneId>>> {
        let docked = self.update_dock(ctx, |dock| dock.undock(pane_id))?;
        self.warpium_bottom_dock.chip_states.remove(&pane_id);
        if self.warpium_bottom_dock.menu_pane == Some(pane_id) {
            self.close_sub_tab_menu(ctx);
        }
        self.focus_state.update(ctx, |focus_state, _| {
            focus_state.set_pane_docked(pane_id, false);
        });
        ctx.notify();
        Some(docked.restore_hint)
    }

    /// Moves focus off a docked pane that is leaving: to the panel's newly selected sub-tab while
    /// the panel stays expanded, else to the layout.
    fn focus_after_leaving_dock(&mut self, ctx: &mut ViewContext<Self>) {
        let focused = self.focused_pane_id(ctx);
        let target = self
            .warpium_bottom_dock
            .model
            .visible_pane()
            .filter(|id| *id != focused)
            .or_else(|| self.warpium_layout_anchor());
        match target {
            Some(target) => self.focus_pane_by_id(target, ctx),
            None => log::warn!("[Bottom panel] No pane to focus after leaving the panel"),
        }
    }

    /// Moves focus from the panel to the layout pane focused last.
    fn focus_after_leaving_dock_to_layout(&mut self, ctx: &mut ViewContext<Self>) {
        match self.warpium_layout_anchor() {
            Some(target) => self.focus_pane_by_id(target, ctx),
            None => log::warn!("[Bottom panel] No layout pane to focus"),
        }
    }
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
