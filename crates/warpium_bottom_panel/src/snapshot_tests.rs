use super::*;

fn terminal(uuid: &str) -> EntrySnapshot {
    EntrySnapshot {
        kind: EntryKind::Terminal {
            uuid: uuid.to_owned(),
            cwd: None,
        },
        title: None,
        origin: DockOrigin::Manual,
    }
}

#[test]
fn round_trips_through_json() {
    let snapshot = PanelSnapshot {
        height: Some(320.),
        expanded: true,
        maximized: false,
        active: Some(1),
        entries: vec![
            EntrySnapshot {
                kind: EntryKind::Terminal {
                    uuid: "0a0b".to_owned(),
                    cwd: Some("/repo".to_owned()),
                },
                title: Some("Assemble".to_owned()),
                origin: DockOrigin::Command {
                    key: "toolbar:assemble".to_owned(),
                },
            },
            EntrySnapshot {
                kind: EntryKind::Logcat,
                title: None,
                origin: DockOrigin::Logcat,
            },
        ],
    };

    let json = snapshot.to_json().expect("serializes");

    assert_eq!(PanelSnapshot::from_json(&json), Some(snapshot));
}

#[test]
fn missing_fields_take_defaults() {
    let snapshot = PanelSnapshot::from_json(r#"{"entries":[{"kind":"terminal","uuid":"ff"}]}"#);

    assert_eq!(
        snapshot,
        Some(PanelSnapshot {
            height: None,
            expanded: false,
            maximized: false,
            active: None,
            entries: vec![terminal("ff")],
        })
    );
}

#[test]
fn json_that_is_not_a_panel_is_rejected() {
    assert_eq!(PanelSnapshot::from_json("not json"), None);
    assert_eq!(PanelSnapshot::from_json("[1, 2]"), None);
}

#[test]
fn unknown_entry_kinds_are_dropped_and_the_selection_follows_its_sub_tab() {
    let json = r#"{
        "active": 2,
        "entries": [
            {"kind": "terminal", "uuid": "01"},
            {"kind": "emulator"},
            {"kind": "terminal", "uuid": "02"}
        ]
    }"#;

    let snapshot = PanelSnapshot::from_json(json).expect("reads");

    assert_eq!(snapshot.entries, vec![terminal("01"), terminal("02")]);
    assert_eq!(snapshot.active, Some(1));
}

#[test]
fn a_dropped_selected_entry_passes_the_selection_to_the_next_one() {
    let json = r#"{
        "active": 1,
        "entries": [
            {"kind": "terminal", "uuid": "01"},
            {"kind": "emulator"},
            {"kind": "terminal", "uuid": "02"}
        ]
    }"#;

    let snapshot = PanelSnapshot::from_json(json).expect("reads");

    assert_eq!(snapshot.active, Some(1));
}

#[test]
fn an_out_of_range_selection_is_clamped_to_the_last_entry() {
    let snapshot =
        PanelSnapshot::from_json(r#"{"active":5,"entries":[{"kind":"logcat"}]}"#).expect("reads");

    assert_eq!(snapshot.active, Some(0));
}

#[test]
fn a_non_positive_height_is_ignored() {
    let snapshot = PanelSnapshot::from_json(r#"{"height":-4}"#).expect("reads");

    assert_eq!(snapshot.height, None);
}

#[test]
fn hex_round_trips() {
    assert_eq!(encode_hex(&[0, 171, 255]), "00abff");
    assert_eq!(decode_hex("00abff"), Some(vec![0, 171, 255]));
    assert_eq!(decode_hex("00ABFF"), Some(vec![0, 171, 255]));
}

#[test]
fn invalid_hex_is_rejected() {
    assert_eq!(decode_hex("abc"), None);
    assert_eq!(decode_hex("zz"), None);
}
