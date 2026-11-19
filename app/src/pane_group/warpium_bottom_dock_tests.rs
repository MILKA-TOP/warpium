//! Warpium: tests for the bottom panel's pane lifecycle in a real `PaneGroup`.

use std::collections::HashMap;

use settings::Setting as _;
use warpium_bottom_panel::{DockOrigin, EntryKind, PanelSnapshot};
use warpui::{App, AppContext, EntityId, SingletonEntity as _, ViewContext};

use super::{get_newly_created_pane_id, initialize_app, mock_pane_group};
use crate::pane_group::{BottomPanelAction, Direction, PaneGroup, PaneId, TerminalPane};
use crate::warpium::sync_settings::WarpiumBottomPanelSettings;

fn terminal_manager_id(panes: &PaneGroup, pane_id: PaneId, ctx: &AppContext) -> EntityId {
    panes
        .downcast_pane_by_id::<TerminalPane>(pane_id)
        .expect("pane should be a terminal pane")
        .terminal_manager(ctx)
        .id()
}

/// Splits the initial terminal and returns (layout pane, second pane).
fn two_terminals(panes: &mut PaneGroup, ctx: &mut ViewContext<PaneGroup>) -> (PaneId, PaneId) {
    let first = get_newly_created_pane_id(panes, &[]);
    panes.add_terminal_pane(Direction::Right, None, ctx);
    let second = get_newly_created_pane_id(panes, &[first]);
    (first, second)
}

fn first_terminal_uuid(snapshot: &PanelSnapshot) -> Option<String> {
    match &snapshot.entries.first()?.kind {
        EntryKind::Terminal { uuid, .. } => Some(uuid.clone()),
        EntryKind::Logcat => None,
    }
}

fn set_bottom_panel_setting(enabled: bool, ctx: &mut ViewContext<PaneGroup>) {
    WarpiumBottomPanelSettings::handle(ctx).update(ctx, |settings, ctx| {
        settings
            .bottom_panel
            .set_value(enabled, ctx)
            .expect("setting should save");
    });
}

#[test]
fn docked_pane_leaves_the_layout_but_keeps_its_terminal_manager() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let (first, second) = two_terminals(panes, ctx);
            let manager_before = terminal_manager_id(panes, second, ctx);

            assert!(panes.move_pane_to_bottom_panel(second, ctx));

            assert_eq!(panes.visible_pane_ids(), vec![first]);
            assert!(panes.has_pane_id(second));
            assert!(panes.is_pane_docked(second));
            assert_eq!(terminal_manager_id(panes, second, ctx), manager_before);
        });
    });
}

#[test]
fn restored_pane_returns_next_to_its_previous_neighbor() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let (first, second) = two_terminals(panes, ctx);
            panes.move_pane_to_bottom_panel(second, ctx);

            assert!(panes.restore_pane_from_bottom_panel(second, ctx));

            assert_eq!(panes.visible_pane_ids(), vec![first, second]);
            assert!(!panes.is_pane_docked(second));
            assert_eq!(panes.focused_pane_id(ctx), second);
        });
    });
}

#[test]
fn closing_a_docked_pane_keeps_the_layout() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let (first, second) = two_terminals(panes, ctx);
            panes.move_pane_to_bottom_panel(second, ctx);

            panes.close_pane(second, ctx);

            assert!(!panes.has_pane_id(second));
            assert!(!panes.is_pane_docked(second));
            assert_eq!(panes.visible_pane_ids(), vec![first]);
        });
    });
}

#[test]
fn closing_the_last_layout_pane_with_a_docked_pane_opens_a_fresh_terminal() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let (first, second) = two_terminals(panes, ctx);
            panes.move_pane_to_bottom_panel(second, ctx);

            panes.close_pane(first, ctx);

            let visible = panes.visible_pane_ids();
            assert_eq!(visible.len(), 1);
            assert_ne!(visible[0], first);
            assert_ne!(visible[0], second);
            assert!(panes.is_pane_docked(second));
        });
    });
}

#[test]
fn moving_the_only_layout_pane_opens_a_fresh_terminal() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first = get_newly_created_pane_id(panes, &[]);

            assert!(panes.move_pane_to_bottom_panel(first, ctx));

            let visible = panes.visible_pane_ids();
            assert_eq!(visible.len(), 1);
            assert_ne!(visible[0], first);
            assert!(panes.is_pane_docked(first));
        });
    });
}

#[test]
fn splitting_from_a_docked_pane_splits_the_layout() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let (first, second) = two_terminals(panes, ctx);
            panes.move_pane_to_bottom_panel(second, ctx);
            panes.focus_pane(second, true, ctx);

            panes.add_terminal_pane(Direction::Right, None, ctx);

            let visible = panes.visible_pane_ids();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0], first);
            assert!(panes.is_pane_docked(second));
        });
    });
}

#[test]
fn moving_is_refused_when_the_setting_is_off() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            set_bottom_panel_setting(false, ctx);
            let (first, second) = two_terminals(panes, ctx);

            assert!(!panes.move_pane_to_bottom_panel(second, ctx));

            assert_eq!(panes.visible_pane_ids(), vec![first, second]);
        });
    });
}

#[test]
fn turning_the_setting_off_restores_docked_panes_to_the_layout() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let second = pane_group.update(&mut app, |panes, ctx| {
            let (_, second) = two_terminals(panes, ctx);
            panes.move_pane_to_bottom_panel(second, ctx);
            second
        });

        pane_group.update(&mut app, |_, ctx| set_bottom_panel_setting(false, ctx));

        pane_group.read(&app, |panes, _| {
            assert!(!panes.is_pane_docked(second));
            assert!(panes.visible_pane_ids().contains(&second));
        });
    });
}

#[test]
fn toggling_collapses_the_panel_and_returns_focus_to_the_layout() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let (first, second) = two_terminals(panes, ctx);
            panes.move_pane_to_bottom_panel(second, ctx);
            panes.focus_pane_by_id(second, ctx);

            panes.handle_warpium_bottom_panel_action(&BottomPanelAction::Toggle, ctx);

            assert!(panes.is_pane_docked(second));
            assert!(panes.warpium_bottom_dock_model().is_collapsed());
            assert_eq!(panes.focused_pane_id(ctx), first);
        });
    });
}

#[test]
fn toggling_a_collapsed_panel_expands_and_focuses_its_sub_tab() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let (_, second) = two_terminals(panes, ctx);
            panes.move_pane_to_bottom_panel(second, ctx);
            panes.handle_warpium_bottom_panel_action(&BottomPanelAction::Collapse, ctx);

            panes.handle_warpium_bottom_panel_action(&BottomPanelAction::Toggle, ctx);

            assert_eq!(
                panes.warpium_bottom_dock_model().visible_pane(),
                Some(second)
            );
            assert_eq!(panes.focused_pane_id(ctx), second);
        });
    });
}

#[test]
fn toggling_an_empty_panel_opens_a_terminal_in_it() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first = get_newly_created_pane_id(panes, &[]);

            panes.handle_warpium_bottom_panel_action(&BottomPanelAction::Toggle, ctx);

            let docked = get_newly_created_pane_id(panes, &[first]);
            assert!(panes.is_pane_docked(docked));
            assert_eq!(panes.visible_pane_ids(), vec![first]);
            assert_eq!(panes.focused_pane_id(ctx), docked);
        });
    });
}

#[test]
fn next_sub_tab_selects_and_focuses_it() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let (first, second) = two_terminals(panes, ctx);
            panes.add_terminal_pane(Direction::Right, None, ctx);
            let third = get_newly_created_pane_id(panes, &[first, second]);
            panes.move_pane_to_bottom_panel(second, ctx);
            panes.move_pane_to_bottom_panel(third, ctx);

            panes.handle_warpium_bottom_panel_action(&BottomPanelAction::SelectNext, ctx);

            assert_eq!(
                panes.warpium_bottom_dock_model().visible_pane(),
                Some(second)
            );
            assert_eq!(panes.focused_pane_id(ctx), second);
        });
    });
}

#[test]
fn maximizing_keeps_the_layout_panes_alive() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let (first, second) = two_terminals(panes, ctx);
            panes.move_pane_to_bottom_panel(second, ctx);

            panes.handle_warpium_bottom_panel_action(&BottomPanelAction::ToggleMaximized, ctx);

            assert!(panes.is_warpium_bottom_panel_maximized());
            assert_eq!(panes.visible_pane_ids(), vec![first]);
        });
    });
}

#[test]
fn a_docked_pane_taken_for_a_move_keeps_its_sub_tab_title_and_origin() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let (first, second) = two_terminals(panes, ctx);
            panes.move_pane_to_bottom_panel(second, ctx);
            let manager = terminal_manager_id(panes, second, ctx);

            let (pane, entry) = panes
                .take_docked_pane_for_move(second, ctx)
                .expect("docked pane should be taken");
            assert!(!panes.has_pane_id(second));
            assert_eq!(panes.visible_pane_ids(), vec![first]);

            let adopted = panes
                .adopt_pane_into_bottom_panel(pane, entry.origin, Some("Build".to_owned()), ctx)
                .expect("pane should be adopted");

            assert!(panes.is_pane_docked(adopted));
            assert_eq!(terminal_manager_id(panes, adopted, ctx), manager);
            assert_eq!(
                panes
                    .warpium_bottom_dock
                    .model
                    .entry(adopted)
                    .and_then(|entry| entry.title.clone()),
                Some("Build".to_owned())
            );
        });
    });
}

#[test]
fn layout_panes_are_not_taken_as_docked_panes() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let (_, second) = two_terminals(panes, ctx);

            assert!(panes.take_docked_pane_for_move(second, ctx).is_none());
            assert!(panes.has_pane_id(second));
        });
    });
}

#[test]
fn command_sub_tabs_are_titled_after_the_command() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first = get_newly_created_pane_id(panes, &[]);
            let cwd = std::env::temp_dir();

            panes
                .add_bottom_panel_session_for_command(
                    "toolbar:build",
                    "Build",
                    cwd,
                    HashMap::new(),
                    ctx,
                )
                .expect("terminal should open");

            let docked = get_newly_created_pane_id(panes, &[first]);
            let entry = panes
                .warpium_bottom_dock
                .model
                .entry(docked)
                .cloned()
                .expect("terminal should be docked");
            assert_eq!(entry.title.as_deref(), Some("Build"));
            assert_eq!(
                entry.origin,
                DockOrigin::Command {
                    key: "toolbar:build".to_owned()
                }
            );
            // Running a command doesn't take focus from the layout.
            assert_eq!(panes.focused_pane_id(ctx), first);
        });
    });
}

#[test]
fn snapshot_lists_docked_terminals_in_sub_tab_order() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            assert_eq!(panes.warpium_bottom_panel_snapshot(ctx), None);
            let (_, second) = two_terminals(panes, ctx);
            panes.move_pane_to_bottom_panel(second, ctx);

            let snapshot = panes
                .warpium_bottom_panel_snapshot(ctx)
                .expect("a docked pane should be saved");

            assert_eq!(snapshot.entries.len(), 1);
            assert!(matches!(
                snapshot.entries[0].kind,
                EntryKind::Terminal { .. }
            ));
            assert_eq!(snapshot.active, Some(0));
            assert!(snapshot.expanded);
        });
    });
}

#[test]
fn restoring_a_snapshot_docks_its_terminals_with_the_saved_uuid() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let source = mock_pane_group(&mut app, Default::default());
        let snapshot = source.update(&mut app, |panes, ctx| {
            let (_, second) = two_terminals(panes, ctx);
            panes.move_pane_to_bottom_panel(second, ctx);
            panes.handle_warpium_bottom_panel_action(&BottomPanelAction::Collapse, ctx);
            panes.warpium_bottom_panel_snapshot(ctx).expect("saved")
        });
        let restored = mock_pane_group(&mut app, Default::default());

        restored.update(&mut app, |panes, ctx| {
            let first = get_newly_created_pane_id(panes, &[]);

            panes.restore_warpium_bottom_panel(&snapshot, &HashMap::new(), ctx);

            let docked = get_newly_created_pane_id(panes, &[first]);
            assert!(panes.is_pane_docked(docked));
            assert!(panes.warpium_bottom_dock_model().is_collapsed());
            assert_eq!(panes.visible_pane_ids(), vec![first]);
            let saved_again = panes.warpium_bottom_panel_snapshot(ctx).expect("saved");
            assert_eq!(
                first_terminal_uuid(&saved_again),
                first_terminal_uuid(&snapshot)
            );
            assert!(first_terminal_uuid(&snapshot).is_some());
        });
    });
}
