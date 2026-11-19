use super::*;
use crate::layout::Side;

fn dock_of(ids: &[u32]) -> BottomDock<u32> {
    let mut dock = BottomDock::default();
    for id in ids {
        dock.dock(*id, None);
    }
    dock
}

#[test]
fn new_dock_is_empty_and_closed() {
    let dock = BottomDock::<u32>::default();

    assert!(dock.is_empty());
    assert!(!dock.is_open());
    assert_eq!(dock.visible_pane(), None);
}

#[test]
fn docking_appends_selects_and_opens() {
    let dock = dock_of(&[1, 2]);

    assert_eq!(dock.ids().collect::<Vec<_>>(), vec![1, 2]);
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

    dock.dock(1, Some(hint));

    assert_eq!(dock.ids().collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(dock.active(), Some(1));
    assert_eq!(
        dock.undock(1),
        Some(DockedPane {
            id: 1,
            restore_hint: Some(hint)
        })
    );
}

#[test]
fn undock_returns_the_restore_hint() {
    let mut dock = BottomDock::default();
    let hint = RestoreHint {
        anchor: 9,
        side: Side::Up,
    };
    dock.dock(3, Some(hint));

    let undocked = dock.undock(3);

    assert_eq!(
        undocked,
        Some(DockedPane {
            id: 3,
            restore_hint: Some(hint)
        })
    );
}

#[test]
fn undocking_the_active_middle_tab_selects_the_next_one() {
    let mut dock = dock_of(&[1, 2, 3]);
    dock.select(2);

    dock.undock(2);

    assert_eq!(dock.active(), Some(3));
}

#[test]
fn undocking_the_active_last_tab_selects_the_previous_one() {
    let mut dock = dock_of(&[1, 2, 3]);

    dock.undock(3);

    assert_eq!(dock.active(), Some(2));
}

#[test]
fn undocking_an_inactive_tab_keeps_the_selection() {
    let mut dock = dock_of(&[1, 2, 3]);

    dock.undock(1);

    assert_eq!(dock.active(), Some(3));
    assert_eq!(dock.ids().collect::<Vec<_>>(), vec![2, 3]);
}

#[test]
fn undocking_the_only_tab_empties_and_closes_the_panel() {
    let mut dock = dock_of(&[1]);

    dock.undock(1);

    assert!(dock.is_empty());
    assert_eq!(dock.active(), None);
    assert!(!dock.is_open());
}

#[test]
fn undocking_an_unknown_pane_changes_nothing() {
    let mut dock = dock_of(&[1, 2]);

    assert_eq!(dock.undock(5), None);
    assert_eq!(dock, dock_of(&[1, 2]));
}

#[test]
fn hidden_panel_keeps_its_selection_but_shows_nothing() {
    let mut dock = dock_of(&[1, 2]);

    dock.set_open(false);

    assert_eq!(dock.active(), Some(2));
    assert_eq!(dock.visible_pane(), None);
}

#[test]
fn selecting_a_docked_pane_opens_the_panel() {
    let mut dock = dock_of(&[1, 2]);
    dock.set_open(false);

    assert!(dock.select(1));

    assert_eq!(dock.visible_pane(), Some(1));
}

#[test]
fn selecting_an_unknown_pane_is_refused() {
    let mut dock = dock_of(&[1, 2]);
    dock.set_open(false);

    assert!(!dock.select(4));

    assert_eq!(dock.active(), Some(2));
    assert!(!dock.is_open());
}

#[test]
fn empty_panel_cannot_be_opened() {
    let mut dock = BottomDock::<u32>::default();

    dock.set_open(true);

    assert!(!dock.is_open());
}
