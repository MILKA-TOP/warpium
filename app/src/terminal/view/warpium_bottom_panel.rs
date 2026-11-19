//! Warpium: the bottom panel's items in the terminal's context and pane header menus, and what its
//! sub-tab chips show about a terminal.

use warpium_bottom_panel::SubTabStatus;
use warpui::AppContext;

use super::TerminalAction;
use crate::menu::{MenuItem, MenuItemFields};
use crate::pane_group::BottomPanelPaneRequest;
use crate::terminal::TerminalView;

impl TerminalView {
    /// "Move to bottom panel" for a pane in the layout, "Restore pane to layout" for a docked one.
    pub(super) fn warpium_bottom_panel_menu_items(
        &self,
        ctx: &AppContext,
    ) -> Vec<MenuItem<TerminalAction>> {
        if !crate::warpium::bottom_panel::is_enabled(ctx) {
            return Vec::new();
        }
        let Some(focus_handle) = self.focus_handle.as_ref() else {
            return Vec::new();
        };
        let (label, request) = if focus_handle.is_docked(ctx) {
            (
                "Restore pane to layout",
                BottomPanelPaneRequest::RestoreToLayout,
            )
        } else {
            (
                "Move to bottom panel",
                BottomPanelPaneRequest::MoveToBottomPanel,
            )
        };
        vec![
            MenuItemFields::new(label)
                .with_on_select_action(TerminalAction::WarpiumBottomPanel(request))
                .into_item(),
        ]
    }

    /// Counts terminal wakeups (new output, a command starting or finishing), so the bottom panel
    /// can tell a hidden sub-tab got output since the user last saw it.
    pub(crate) fn warpium_output_generation(&self) -> u64 {
        self.warpium_output_generation
    }

    /// The chip status: running, or the last finished command's exit code.
    pub(crate) fn warpium_sub_tab_status(&self) -> SubTabStatus {
        let model = self.model.lock();
        let block_list = model.block_list();
        let running =
            block_list.active_block().is_active_and_long_running() && !model.is_read_only();
        let last_exit_code = block_list
            .last_non_hidden_block()
            .filter(|block| block.finished() && !block.is_background())
            .map(|block| block.exit_code().value());
        SubTabStatus::of_terminal(running, last_exit_code)
    }

    /// Whether a new command can't be typed into this session now: something runs, the user has
    /// typed something, or it isn't local.
    pub(crate) fn warpium_is_busy(&self, ctx: &AppContext) -> bool {
        let is_executing = self.model.lock().block_list().active_block().is_executing();
        is_executing
            || !self.active_session_is_local(ctx).unwrap_or(false)
            || !self.input().as_ref(ctx).buffer_text(ctx).trim().is_empty()
    }
}
