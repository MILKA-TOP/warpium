//! Warpium: the bottom panel's items in the terminal's context and pane header menus.

use warpui::AppContext;

use super::TerminalAction;
use crate::features::FeatureFlag;
use crate::menu::{MenuItem, MenuItemFields};
use crate::pane_group::BottomPanelPaneRequest;
use crate::terminal::TerminalView;

impl TerminalView {
    /// "Move to bottom panel" for a pane in the layout, "Restore pane to layout" for a docked one.
    pub(super) fn warpium_bottom_panel_menu_items(
        &self,
        ctx: &AppContext,
    ) -> Vec<MenuItem<TerminalAction>> {
        if !FeatureFlag::WarpiumBottomPanel.is_enabled() {
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
}
