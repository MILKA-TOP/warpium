use std::fs;
use std::path::Path;

use super::*;
use crate::model::{AndroidLaunch, StepAction};

fn project() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("settings.gradle.kts"), "").unwrap();
    fs::write(root.path().join("build.gradle.kts"), "").unwrap();
    fs::write(root.path().join("gradlew"), "").unwrap();
    fs::create_dir_all(root.path().join("app")).unwrap();
    fs::write(root.path().join("app/build.gradle.kts"), "").unwrap();
    root
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn gradle(name: &str, task: &str, extra_attributes: &str) -> String {
    format!(
        r#"<configuration name="{name}" type="GradleRunConfiguration" factoryName="Gradle" {extra_attributes}>
    <ExternalSystemSettings>
      <option name="externalProjectPath" value="$PROJECT_DIR$" />
      <option name="taskNames"><list><option value="{task}" /></list></option>
    </ExternalSystemSettings>
  </configuration>"#
    )
}

fn shared_file(configuration: &str) -> String {
    format!(
        r#"<component name="ProjectRunConfigurationManager">
  {configuration}
</component>"#
    )
}

fn workspace(selected: &str, configurations: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<project version="4">
  <component name="ChangeListManager" />
  <component name="RunManager" selected="{selected}">
    {configurations}
    <configuration default="true" type="GradleRunConfiguration" factoryName="Gradle">
      <ExternalSystemSettings />
    </configuration>
    <recent_temporary>
      <list><item itemvalue="Gradle.test once" /></list>
    </recent_temporary>
  </component>
</project>"#
    )
}

fn summary(configs: &IdeRunConfigs) -> Vec<(String, ConfigOrigin, bool)> {
    configs
        .configs
        .iter()
        .map(|config| {
            (
                config.name.clone(),
                config.origin.clone(),
                config.steps().is_some(),
            )
        })
        .collect()
}

#[test]
fn shared_and_personal_configurations_are_read_in_order() {
    let root = project();
    let root = root.path();
    write(
        root,
        ".run/lint.run.xml",
        &shared_file(&gradle("lint", "lint", "")),
    );
    write(
        root,
        ".run/b.run.xml",
        &shared_file(&gradle("assemble", "assemble", "")),
    );
    write(root, ".run/notes.txt", "not a run configuration");
    write(
        root,
        ".idea/runConfigurations/detekt.xml",
        &shared_file(&gradle("detekt", "detekt", "")),
    );
    write(
        root,
        WORKSPACE_FILE,
        &workspace(
            "Gradle.mine",
            &format!(
                "{}{}",
                gradle("mine", "check", ""),
                gradle("test once", "test", r#"temporary="true""#)
            ),
        ),
    );

    let configs = load_project(root, None);

    assert_eq!(
        summary(&configs),
        vec![
            (
                "assemble".to_owned(),
                ConfigOrigin::Shared(".run/b.run.xml".to_owned()),
                true
            ),
            (
                "lint".to_owned(),
                ConfigOrigin::Shared(".run/lint.run.xml".to_owned()),
                true
            ),
            (
                "detekt".to_owned(),
                ConfigOrigin::Shared(".idea/runConfigurations/detekt.xml".to_owned()),
                true
            ),
            ("mine".to_owned(), ConfigOrigin::Personal, true),
        ]
    );
    assert_eq!(
        configs.default_key.as_deref(),
        Some("ide:GradleRunConfiguration:mine")
    );
    assert!(configs.warnings.is_empty(), "{:?}", configs.warnings);
}

#[test]
fn temporary_configurations_are_kept_when_there_is_nothing_else() {
    let root = project();
    write(
        root.path(),
        WORKSPACE_FILE,
        &workspace(
            "Gradle.test once",
            &gradle("test once", "test", r#"temporary="true""#),
        ),
    );
    let configs = load_project(root.path(), None);
    assert_eq!(
        summary(&configs),
        vec![("test once".to_owned(), ConfigOrigin::Personal, true)]
    );
    assert_eq!(
        configs.default_key.as_deref(),
        Some("ide:GradleRunConfiguration:test once")
    );
}

#[test]
fn a_selected_temporary_configuration_is_not_the_default_when_it_is_left_out() {
    let root = project();
    write(
        root.path(),
        WORKSPACE_FILE,
        &workspace(
            "Gradle.test once",
            &format!(
                "{}{}",
                gradle("mine", "check", ""),
                gradle("test once", "test", r#"temporary="true""#)
            ),
        ),
    );
    assert_eq!(load_project(root.path(), None).default_key, None);
}

#[test]
fn broken_files_and_configurations_become_warnings() {
    let root = project();
    let root = root.path();
    write(root, ".run/broken.run.xml", "<component><configuration");
    write(
        root,
        ".run/nameless.run.xml",
        &shared_file(r#"<configuration type="GradleRunConfiguration" />"#),
    );
    write(
        root,
        ".run/ok.run.xml",
        &shared_file(&gradle("ok", "build", "")),
    );

    let configs = load_project(root, None);

    assert_eq!(
        summary(&configs),
        vec![(
            "ok".to_owned(),
            ConfigOrigin::Shared(".run/ok.run.xml".to_owned()),
            true
        )]
    );
    assert_eq!(configs.warnings.len(), 2, "{:?}", configs.warnings);
    assert!(configs.warnings[0].starts_with(".run/broken.run.xml: invalid XML"));
    assert!(configs.warnings[1].starts_with(".run/nameless.run.xml: line 2"));
}

#[test]
fn a_bare_configuration_file_is_read() {
    let root = project();
    write(
        root.path(),
        ".run/bare.run.xml",
        &gradle("bare", "build", ""),
    );
    assert_eq!(load_project(root.path(), None).configs.len(), 1);
}

#[test]
fn duplicates_keep_the_first_configuration() {
    let root = project();
    write(
        root.path(),
        ".run/a.run.xml",
        &shared_file(&gradle("same", "a", "")),
    );
    write(
        root.path(),
        WORKSPACE_FILE,
        &workspace("", &gradle("same", "b", "")),
    );
    let configs = load_project(root.path(), None);
    assert_eq!(configs.configs.len(), 1);
    assert!(configs.configs[0].origin.is_shared());
}

#[test]
fn compound_configurations_run_their_members_in_order() {
    let root = project();
    write(
        root.path(),
        ".run/all.run.xml",
        &shared_file(
            r#"<configuration name="all" type="CompoundRunConfigurationType">
    <toRun name="lint" type="GradleRunConfiguration" />
    <toRun name="app" type="AndroidRunConfigurationType" />
  </configuration>"#,
        ),
    );
    write(
        root.path(),
        ".run/lint.run.xml",
        &shared_file(&gradle("lint", "lint", "")),
    );
    write(
        root.path(),
        WORKSPACE_FILE,
        &workspace(
            "Compound.all",
            r#"<configuration name="app" type="AndroidRunConfigurationType">
      <module name="MyApp.app" />
      <option name="MODE" value="specific_activity" />
      <option name="ACTIVITY_CLASS" value=".MainActivity" />
    </configuration>"#,
        ),
    );

    let configs = load_project(root.path(), None);
    let all = configs.get("ide:CompoundRunConfigurationType:all").unwrap();
    let steps = all.steps().unwrap();

    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].config_key, "ide:GradleRunConfiguration:lint");
    assert!(steps[0].shared);
    assert!(
        matches!(&steps[0].action, StepAction::Command(command) if command.command == "./gradlew lint")
    );
    assert_eq!(steps[1].label, "app");
    assert!(!steps[1].shared);
    assert!(matches!(
        &steps[1].action,
        StepAction::AndroidApp(run)
            if run.gradle_path == ":app"
                && run.launch == AndroidLaunch::Activity(".MainActivity".to_owned())
    ));
    assert_eq!(
        configs.default_key.as_deref(),
        Some("ide:CompoundRunConfigurationType:all")
    );
}

#[test]
fn compound_with_a_missing_or_unsupported_member_is_unsupported() {
    let root = project();
    write(
        root.path(),
        ".run/c.run.xml",
        &shared_file(
            r#"<configuration name="missing" type="CompoundRunConfigurationType">
    <toRun name="nope" type="GradleRunConfiguration" />
  </configuration>
  <configuration name="kotlin" type="CompoundRunConfigurationType">
    <toRun name="Main" type="JetRunConfigurationType" />
  </configuration>
  <configuration name="Main" type="JetRunConfigurationType" />
  <configuration name="empty" type="CompoundRunConfigurationType" />
  <configuration name="loop" type="CompoundRunConfigurationType">
    <toRun name="loop" type="CompoundRunConfigurationType" />
  </configuration>"#,
        ),
    );
    let configs = load_project(root.path(), None);
    let reason = |name: &str| match &configs
        .get(&IdeRunConfig::key_for("CompoundRunConfigurationType", name))
        .unwrap()
        .run
    {
        IdeRun::Unsupported(reason) => reason.clone(),
        IdeRun::Steps(_) => panic!("{name} should be unsupported"),
    };
    assert!(reason("missing").contains("\"nope\" wasn't found"));
    assert!(reason("kotlin").starts_with("its member \"Main\":"));
    assert!(reason("empty").contains("no members"));
    assert!(reason("loop").contains("nested too deeply"));
}

#[test]
fn the_ide_selection_matches_by_name_after_the_type_prefix() {
    let root = project();
    write(
        root.path(),
        WORKSPACE_FILE,
        &workspace(
            "Android App.app",
            &format!(
                "{}{}",
                gradle("app", "a", ""),
                r#"<configuration name="app" type="AndroidRunConfigurationType"><module name="MyApp.app" /></configuration>"#
            ),
        ),
    );
    // Either named `app` would be a reasonable match; the default is one of them and runnable.
    let configs = load_project(root.path(), None);
    let default = configs.default_key.unwrap();
    assert!(default.ends_with(":app"), "{default}");
}

#[test]
fn a_workspace_file_tracked_by_git_counts_as_shared() {
    let root = project();
    write(
        root.path(),
        WORKSPACE_FILE,
        &workspace("", &gradle("mine", "check", "")),
    );
    assert_eq!(
        load_project(root.path(), None).configs[0].origin,
        ConfigOrigin::Personal
    );
    // Index entries store the path followed by a NUL.
    write(
        root.path(),
        ".git/index",
        "DIRC\0\0\0\x02junk.idea/workspace.xml\0more",
    );
    assert_eq!(
        load_project(root.path(), None).configs[0].origin,
        ConfigOrigin::Shared(WORKSPACE_FILE.to_owned())
    );
}

#[test]
fn a_worktree_git_file_points_at_the_index() {
    let root = project();
    let git_dir = tempfile::tempdir().unwrap();
    fs::write(git_dir.path().join("index"), "x.idea/workspace.xml\0").unwrap();
    write(
        root.path(),
        ".git",
        &format!("gitdir: {}\n", git_dir.path().display()),
    );
    write(
        root.path(),
        WORKSPACE_FILE,
        &workspace("", &gradle("mine", "check", "")),
    );
    assert!(
        load_project(root.path(), None).configs[0]
            .origin
            .is_shared()
    );
}

#[test]
fn ide_config_paths_are_recognized() {
    let root = Path::new("/work/app");
    assert!(is_ide_config_path(root, Path::new("/work/app/.run")));
    assert!(is_ide_config_path(
        root,
        Path::new("/work/app/.run/a.run.xml")
    ));
    assert!(is_ide_config_path(root, Path::new("/work/app/.idea")));
    assert!(is_ide_config_path(
        root,
        Path::new("/work/app/.idea/workspace.xml")
    ));
    assert!(is_ide_config_path(
        root,
        Path::new("/work/app/.idea/runConfigurations/a.xml")
    ));
    assert!(!is_ide_config_path(
        root,
        Path::new("/work/app/.idea/misc.xml")
    ));
    assert!(!is_ide_config_path(
        root,
        Path::new("/work/app/build.gradle.kts")
    ));
    assert_eq!(watched_dirs(root).len(), 3);
}
