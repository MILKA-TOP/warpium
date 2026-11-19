//! Warpium: the bottom panel's setting and its window-wide shortcuts and palette commands.
//!
//! The panel itself lives in each tab's `PaneGroup` (`pane_group/warpium_bottom_dock.rs`); the
//! model is the `warpium_bottom_panel` crate. These bindings sit on the workspace so they also work
//! while focus is outside the tab's panes (the Project Explorer, say), and act on the active tab.

use warpui::keymap::EditableBinding;
use warpui::{AppContext, SingletonEntity as _};

use super::sync_settings::{BOTTOM_PANEL_CONTEXT_FLAG, WarpiumBottomPanelSettings};
use crate::pane_group::BottomPanelAction;
use crate::settings_view::ToggleSettingActionPair;
use crate::util::bindings::BindingGroup;
use crate::workspace::WorkspaceAction;

/// Whether the bottom panel is available: `warpium.bottom_panel` is on.
pub(crate) fn is_enabled(ctx: &AppContext) -> bool {
    ctx.has_singleton_model::<WarpiumBottomPanelSettings>()
        && *WarpiumBottomPanelSettings::as_ref(ctx).bottom_panel
}

pub(crate) fn init_bindings(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    let enabled = id!("Workspace") & id!(BOTTOM_PANEL_CONTEXT_FLAG);
    let action = WorkspaceAction::WarpiumBottomPanel;
    app.register_editable_bindings([
        // Like VS Code's "Toggle Panel".
        EditableBinding::new(
            "workspace:toggle_bottom_panel",
            "Toggle bottom panel",
            action(BottomPanelAction::Toggle),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(enabled.clone())
        .with_mac_key_binding("cmd-j"),
        EditableBinding::new(
            "workspace:new_terminal_in_bottom_panel",
            "New terminal in bottom panel",
            action(BottomPanelAction::NewTerminal),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(enabled.clone()),
        // `cmd-alt-[`/`]` fold code in the editor, so these use ctrl.
        EditableBinding::new(
            "workspace:bottom_panel_next_tab",
            "Bottom panel: next sub-tab",
            action(BottomPanelAction::SelectNext),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(enabled.clone())
        .with_mac_key_binding("cmd-ctrl-]"),
        EditableBinding::new(
            "workspace:bottom_panel_previous_tab",
            "Bottom panel: previous sub-tab",
            action(BottomPanelAction::SelectPrevious),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(enabled.clone())
        .with_mac_key_binding("cmd-ctrl-["),
        // `cmd-shift-J` queues the next agent prompt in terminals.
        EditableBinding::new(
            "workspace:maximize_bottom_panel",
            "Maximize or restore bottom panel",
            action(BottomPanelAction::ToggleMaximized),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(enabled)
        .with_mac_key_binding("cmd-ctrl-j"),
    ]);

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "bottom panel",
            WorkspaceAction::ToggleWarpiumBottomPanelSetting,
            &id!("Workspace"),
            BOTTOM_PANEL_CONTEXT_FLAG,
        )],
        app,
    );
}
