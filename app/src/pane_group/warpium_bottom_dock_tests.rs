//! Warpium: tests for the bottom panel's pane lifecycle in a real `PaneGroup`.

use warp_core::features::FeatureFlag;
use warpui::{App, AppContext, EntityId};

use super::{get_newly_created_pane_id, initialize_app, mock_pane_group};
use crate::pane_group::{Direction, PaneGroup, PaneId, TerminalPane};

fn terminal_manager_id(panes: &PaneGroup, pane_id: PaneId, ctx: &AppContext) -> EntityId {
    panes
        .downcast_pane_by_id::<TerminalPane>(pane_id)
        .expect("pane should be a terminal pane")
        .terminal_manager(ctx)
        .id()
}

#[test]
fn docked_pane_leaves_the_layout_but_keeps_its_terminal_manager() {
    let _flag = FeatureFlag::WarpiumBottomPanel.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Right, None, ctx);
            let second = get_newly_created_pane_id(panes, &[first]);
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
    let _flag = FeatureFlag::WarpiumBottomPanel.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Right, None, ctx);
            let second = get_newly_created_pane_id(panes, &[first]);
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
    let _flag = FeatureFlag::WarpiumBottomPanel.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Right, None, ctx);
            let second = get_newly_created_pane_id(panes, &[first]);
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
    let _flag = FeatureFlag::WarpiumBottomPanel.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Right, None, ctx);
            let second = get_newly_created_pane_id(panes, &[first]);
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
    let _flag = FeatureFlag::WarpiumBottomPanel.override_enabled(true);
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
    let _flag = FeatureFlag::WarpiumBottomPanel.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Right, None, ctx);
            let second = get_newly_created_pane_id(panes, &[first]);
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
fn moving_is_refused_when_the_flag_is_off() {
    let _flag = FeatureFlag::WarpiumBottomPanel.override_enabled(false);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Right, None, ctx);
            let second = get_newly_created_pane_id(panes, &[first]);

            assert!(!panes.move_pane_to_bottom_panel(second, ctx));

            assert_eq!(panes.visible_pane_ids(), vec![first, second]);
        });
    });
}
