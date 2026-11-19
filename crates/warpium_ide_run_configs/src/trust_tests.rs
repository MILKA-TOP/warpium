use std::collections::BTreeMap;

use warpium_project_toolbar::{CommandAction, CommandTarget, TrustChange};

use super::*;
use crate::model::{AndroidLaunch, AndroidRun, ConfigOrigin, IdeRun};

fn command_config(name: &str, command: &str, shared: bool) -> IdeRunConfig {
    let key = IdeRunConfig::key_for("GradleRunConfiguration", name);
    IdeRunConfig {
        key: key.clone(),
        name: name.to_owned(),
        type_id: "GradleRunConfiguration".to_owned(),
        origin: if shared {
            ConfigOrigin::Shared(format!(".run/{name}.run.xml"))
        } else {
            ConfigOrigin::Personal
        },
        run: IdeRun::Steps(vec![RunStep {
            config_key: key,
            label: name.to_owned(),
            shared,
            action: StepAction::Command(CommandAction {
                command: command.to_owned(),
                target: CommandTarget::Current,
                cwd: Some("app".to_owned()),
                env: BTreeMap::from([("CI".to_owned(), "1".to_owned())]),
            }),
        }]),
    }
}

fn android_config(shared: bool) -> IdeRunConfig {
    let key = IdeRunConfig::key_for("AndroidRunConfigurationType", "app");
    IdeRunConfig {
        key: key.clone(),
        name: "app".to_owned(),
        type_id: "AndroidRunConfigurationType".to_owned(),
        origin: ConfigOrigin::Shared(".run/app.run.xml".to_owned()),
        run: IdeRun::Steps(vec![RunStep {
            config_key: key,
            label: "app".to_owned(),
            shared,
            action: StepAction::AndroidApp(AndroidRun {
                gradle_path: ":app".to_owned(),
                launch: AndroidLaunch::DefaultActivity,
            }),
        }]),
    }
}

fn compound(members: &[&IdeRunConfig]) -> IdeRunConfig {
    IdeRunConfig {
        key: IdeRunConfig::key_for("CompoundRunConfigurationType", "all"),
        name: "all".to_owned(),
        type_id: "CompoundRunConfigurationType".to_owned(),
        origin: ConfigOrigin::Personal,
        run: IdeRun::Steps(
            members
                .iter()
                .flat_map(|member| member.steps().unwrap().to_vec())
                .collect(),
        ),
    }
}

#[test]
fn only_shared_commands_need_trust() {
    let shared = command_config("lint", "./gradlew lint", true);
    assert!(needs_trust(&shared));
    assert!(!needs_trust(&command_config(
        "mine",
        "./gradlew check",
        false
    )));
    assert!(!needs_trust(&android_config(true)));
    assert!(needs_trust(&compound(&[&android_config(true), &shared])));
    let unsupported = IdeRunConfig {
        run: IdeRun::Unsupported("no".to_owned()),
        ..shared
    };
    assert!(!needs_trust(&unsupported));
}

#[test]
fn the_fingerprint_follows_the_shared_commands() {
    let lint = command_config("lint", "./gradlew lint", true);
    let mine = command_config("mine", "./gradlew check", false);
    let fingerprint = trust_fingerprint(&[lint.clone(), mine.clone()]).unwrap();

    assert_eq!(trust_fingerprint(std::slice::from_ref(&mine)), None);
    assert_eq!(
        trust_fingerprint(&[lint.clone(), command_config("mine", "other", false)]),
        Some(fingerprint.clone()),
        "personal commands don't change it"
    );
    assert_ne!(
        trust_fingerprint(&[command_config("lint", "./gradlew lint --fix", true), mine]),
        Some(fingerprint.clone())
    );
    // A compound repeating a member's step doesn't change it either.
    assert_eq!(
        trust_fingerprint(&[lint.clone(), compound(&[&lint])]),
        Some(fingerprint)
    );
}

#[test]
fn the_review_marks_new_changed_unchanged_and_removed_commands() {
    let lint = command_config("lint", "./gradlew lint", true);
    let detekt = command_config("detekt", "./gradlew detekt", true);
    let gone = command_config("gone", "./gradlew gone", true);
    let trusted = trust_signatures(&[lint.clone(), detekt.clone(), gone]);

    let review = trust_review(
        &[
            command_config("detekt", "./gradlew detekt --auto-correct", true),
            lint.clone(),
            command_config("new", "./gradlew new", true),
            command_config("mine", "./gradlew mine", false),
        ],
        Some(&trusted),
    );

    assert!(review.is_update);
    assert_eq!(review.removed, 1);
    assert_eq!(
        review
            .listings
            .iter()
            .map(|listing| (listing.label.as_str(), listing.change))
            .collect::<Vec<_>>(),
        vec![
            ("new", TrustChange::New),
            ("detekt", TrustChange::Changed),
            ("lint", TrustChange::Unchanged),
        ]
    );
    assert_eq!(
        review.listings[2].details,
        "Gradle · .run/lint.run.xml · cwd app · env CI"
    );
    assert_eq!(review.listings[2].command, "./gradlew lint");
}

#[test]
fn a_first_review_lists_everything_as_new() {
    let review = trust_review(&[command_config("lint", "./gradlew lint", true)], None);
    assert!(!review.is_update);
    assert_eq!(review.removed, 0);
    assert_eq!(review.count(TrustChange::New), 1);
}
