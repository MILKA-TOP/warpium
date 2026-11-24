use super::*;

#[test]
fn panel_tab_settings_round_trip() {
    assert_eq!(PanelTab::from_settings("changes"), PanelTab::Changes);
    assert_eq!(PanelTab::from_settings("stashes"), PanelTab::Stashes);
    assert_eq!(PanelTab::from_settings("anything-else"), PanelTab::Changes);
    assert_eq!(PanelTab::Stashes.as_settings_value(), "stashes");
    assert_eq!(PanelTab::default(), PanelTab::Changes);
}

#[test]
fn set_entries_rebuilds_row_interactions() {
    let mut state = WarpiumStashTabState::default();
    state.set_entries(vec![
        warpium_git::stash::StashEntry {
            index: 0,
            message: "one".to_owned(),
            created: String::new(),
            oid: warpium_git::Oid::from_hex("9911e3876c7128dde5476aa0141916d403365415")
                .unwrap(),
        },
        warpium_git::stash::StashEntry {
            index: 1,
            message: "two".to_owned(),
            created: String::new(),
            oid: warpium_git::Oid::from_hex("9911e3876c7128dde5476aa0141916d403365415")
                .unwrap(),
        },
    ]);
    assert_eq!(state.entries.len(), 2);
    assert_eq!(state.row_interactions.len(), 2);
    state.reset();
    assert!(state.entries.is_empty());
    assert!(state.row_interactions.is_empty());
    assert_eq!(state.selected_stash, None);
}
