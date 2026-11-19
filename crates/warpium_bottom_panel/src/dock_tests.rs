use super::*;
use crate::layout::Side;

fn manual(id: u32) -> DockedPane<u32> {
    DockedPane::new(id, DockOrigin::Manual, 0)
}

fn command(id: u32, key: &str) -> DockedPane<u32> {
    DockedPane::new(
        id,
        DockOrigin::Command {
            key: key.to_owned(),
        },
        0,
    )
}

fn dock_of(ids: &[u32]) -> BottomDock<u32> {
    let mut dock = BottomDock::default();
    for id in ids {
        dock.dock(manual(*id));
    }
    dock
}

fn ids(dock: &BottomDock<u32>) -> Vec<u32> {
    dock.ids().collect()
}

#[test]
fn new_dock_is_empty_and_shows_nothing() {
    let dock = BottomDock::<u32>::default();

    assert!(dock.is_empty());
    assert!(!dock.is_expanded());
    assert!(!dock.is_collapsed());
    assert_eq!(dock.visible_pane(), None);
}

#[test]
fn docking_appends_selects_and_expands() {
    let dock = dock_of(&[1, 2]);

    assert_eq!(ids(&dock), vec![1, 2]);
    assert_eq!(dock.active(), Some(2));
    assert_eq!(dock.visible_pane(), Some(2));
}

#[test]
fn docking_an_already_docked_pane_keeps_its_place_and_updates_its_hint() {
    let mut dock = dock_of(&[1, 2]);
    let hint = RestoreHint {
        anchor: 7,
        side: Side::Right,
    };

    dock.dock(manual(1).with_restore_hint(Some(hint)));

    assert_eq!(ids(&dock), vec![1, 2]);
    assert_eq!(dock.active(), Some(1));
    assert_eq!(
        dock.undock(1).and_then(|entry| entry.restore_hint),
        Some(hint)
    );
}

#[test]
fn undocking_the_selected_pane_selects_the_next_one() {
    let mut dock = dock_of(&[1, 2, 3]);
    dock.select(2);

    dock.undock(2);

    assert_eq!(dock.active(), Some(3));
}

#[test]
fn undocking_the_last_selected_pane_selects_the_previous_one() {
    let mut dock = dock_of(&[1, 2, 3]);

    dock.undock(3);

    assert_eq!(dock.active(), Some(2));
}

#[test]
fn undocking_another_pane_keeps_the_selection() {
    let mut dock = dock_of(&[1, 2, 3]);

    dock.undock(1);

    assert_eq!(dock.active(), Some(3));
}

#[test]
fn undocking_the_only_pane_empties_and_resets_the_panel() {
    let mut dock = dock_of(&[1]);
    dock.toggle_maximized();

    dock.undock(1);

    assert!(dock.is_empty());
    assert_eq!(dock.active(), None);
    assert!(!dock.is_expanded());
    assert!(!dock.is_maximized());
}

#[test]
fn undocking_an_unknown_pane_returns_none() {
    let mut dock = dock_of(&[1]);

    assert_eq!(dock.undock(5), None);
    assert_eq!(ids(&dock), vec![1]);
}

#[test]
fn collapsing_keeps_the_strip_and_the_selection() {
    let mut dock = dock_of(&[1, 2]);

    dock.set_expanded(false);

    assert!(dock.is_collapsed());
    assert_eq!(dock.visible_pane(), None);
    assert_eq!(dock.active(), Some(2));
}

#[test]
fn selecting_a_chip_of_the_collapsed_strip_expands_on_it() {
    let mut dock = dock_of(&[1, 2]);
    dock.set_expanded(false);

    assert!(dock.select(1));

    assert_eq!(dock.visible_pane(), Some(1));
}

#[test]
fn selecting_an_unknown_pane_changes_nothing() {
    let mut dock = dock_of(&[1]);
    dock.set_expanded(false);

    assert!(!dock.select(9));

    assert!(dock.is_collapsed());
}

#[test]
fn an_empty_panel_cannot_expand() {
    let mut dock = BottomDock::<u32>::default();

    dock.set_expanded(true);

    assert!(!dock.is_expanded());
}

#[test]
fn next_and_previous_wrap_around() {
    let mut dock = dock_of(&[1, 2, 3]);

    assert_eq!(dock.select_next(), Some(1));
    assert_eq!(dock.select_previous(), Some(3));
    assert_eq!(dock.select_previous(), Some(2));
}

#[test]
fn next_expands_a_collapsed_panel() {
    let mut dock = dock_of(&[1, 2]);
    dock.set_expanded(false);

    dock.select_next();

    assert!(dock.is_expanded());
}

#[test]
fn next_on_an_empty_panel_selects_nothing() {
    let mut dock = BottomDock::<u32>::default();

    assert_eq!(dock.select_next(), None);
}

#[test]
fn maximizing_expands_and_toggles_back() {
    let mut dock = dock_of(&[1]);
    dock.set_expanded(false);

    assert!(dock.toggle_maximized());
    assert!(dock.is_expanded());
    assert!(dock.is_maximized());
    assert!(!dock.toggle_maximized());
    assert!(!dock.is_maximized());
}

#[test]
fn collapsing_ends_maximizing() {
    let mut dock = dock_of(&[1]);
    dock.toggle_maximized();

    dock.set_expanded(false);
    dock.set_expanded(true);

    assert!(!dock.is_maximized());
}

#[test]
fn an_empty_panel_cannot_maximize() {
    let mut dock = BottomDock::<u32>::default();

    assert!(!dock.toggle_maximized());
}

#[test]
fn moving_an_entry_after_a_target() {
    let mut dock = dock_of(&[1, 2, 3]);

    assert!(dock.move_entry(1, 3, true));

    assert_eq!(ids(&dock), vec![2, 3, 1]);
}

#[test]
fn moving_an_entry_before_a_target() {
    let mut dock = dock_of(&[1, 2, 3]);

    assert!(dock.move_entry(3, 1, false));

    assert_eq!(ids(&dock), vec![3, 1, 2]);
}

#[test]
fn moving_an_entry_onto_itself_or_an_unknown_target_does_nothing() {
    let mut dock = dock_of(&[1, 2]);

    assert!(!dock.move_entry(1, 1, true));
    assert!(!dock.move_entry(1, 9, true));
    assert!(!dock.move_entry(9, 1, true));
    assert_eq!(ids(&dock), vec![1, 2]);
}

#[test]
fn moving_an_entry_to_where_it_already_is_reports_no_change() {
    let mut dock = dock_of(&[1, 2]);

    assert!(!dock.move_entry(1, 2, false));
}

#[test]
fn a_command_reuses_its_idle_sub_tab() {
    let mut dock = BottomDock::default();
    dock.dock(command(1, "toolbar:build"));
    dock.dock(command(2, "toolbar:test"));
    let origin = DockOrigin::Command {
        key: "toolbar:build".to_owned(),
    };

    assert_eq!(dock.idle_entry_with_origin(&origin, |_| false), Some(1));
}

#[test]
fn a_command_skips_its_busy_sub_tabs() {
    let mut dock = BottomDock::default();
    dock.dock(command(1, "toolbar:build"));
    dock.dock(command(2, "toolbar:build"));
    let origin = DockOrigin::Command {
        key: "toolbar:build".to_owned(),
    };

    assert_eq!(dock.idle_entry_with_origin(&origin, |id| id == 1), Some(2));
    assert_eq!(dock.idle_entry_with_origin(&origin, |_| true), None);
}

#[test]
fn new_sub_tabs_of_a_busy_command_are_numbered() {
    let mut dock = BottomDock::default();
    let origin = DockOrigin::Command {
        key: "toolbar:build".to_owned(),
    };

    assert_eq!(dock.title_for_new_entry(&origin, "Build"), "Build");
    dock.dock(command(1, "toolbar:build"));
    assert_eq!(dock.title_for_new_entry(&origin, "Build"), "Build (2)");
}

#[test]
fn hidden_output_is_unread_until_marked_seen() {
    let mut dock = BottomDock::default();
    dock.dock(DockedPane::new(1, DockOrigin::Manual, 4));
    dock.dock(DockedPane::new(2, DockOrigin::Manual, 0));

    assert!(!dock.has_unread(1, 4));
    assert!(dock.has_unread(1, 5));
    dock.mark_seen(1, 5);
    assert!(!dock.has_unread(1, 5));
}

#[test]
fn the_visible_sub_tab_is_never_unread() {
    let dock = dock_of(&[1]);

    assert!(!dock.has_unread(1, 100));
}

#[test]
fn a_collapsed_panel_shows_unread_output_of_its_selected_sub_tab() {
    let mut dock = dock_of(&[1]);
    dock.set_expanded(false);

    assert!(dock.has_unread(1, 1));
}

#[test]
fn marking_seen_never_goes_back() {
    let mut dock = BottomDock::default();
    dock.dock(DockedPane::new(1, DockOrigin::Manual, 10));
    dock.set_expanded(false);

    dock.mark_seen(1, 3);

    assert!(!dock.has_unread(1, 10));
}

#[test]
fn restoring_keeps_the_saved_selection() {
    let dock =
        BottomDock::from_restored(vec![Some(manual(1)), Some(manual(2))], Some(1), true, false);

    assert_eq!(ids(&dock), vec![1, 2]);
    assert_eq!(dock.visible_pane(), Some(2));
}

#[test]
fn restoring_selects_the_next_pane_when_the_selected_one_failed() {
    let dock = BottomDock::from_restored(
        vec![Some(manual(1)), None, Some(manual(3))],
        Some(1),
        false,
        false,
    );

    assert_eq!(ids(&dock), vec![1, 3]);
    assert_eq!(dock.active(), Some(3));
    assert!(dock.is_collapsed());
}

#[test]
fn restoring_selects_the_previous_pane_when_the_selected_last_one_failed() {
    let dock = BottomDock::from_restored(vec![Some(manual(1)), None], Some(1), true, true);

    assert_eq!(dock.active(), Some(1));
    assert!(dock.is_maximized());
}

#[test]
fn restoring_nothing_gives_an_empty_panel() {
    let dock = BottomDock::<u32>::from_restored(vec![None], Some(0), true, true);

    assert!(dock.is_empty());
    assert!(!dock.is_expanded());
    assert!(!dock.is_maximized());
}
