use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use roxmltree::Document;
use warpium_project_toolbar::{CommandAction, CommandTarget};

use super::*;

/// A Gradle project with a wrapper and the given module directories.
fn project(dirs: &[&str]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("settings.gradle.kts"), "").unwrap();
    fs::write(root.path().join("build.gradle.kts"), "").unwrap();
    fs::write(root.path().join("gradlew"), "").unwrap();
    for dir in dirs {
        let dir = root.path().join(dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("build.gradle.kts"), "").unwrap();
    }
    root
}

fn map(root: &Path, xml: &str) -> Mapped {
    let document = Document::parse(xml).unwrap();
    let node = document.root_element();
    let type_id = node.attribute("type").unwrap();
    map_configuration(
        node,
        type_id,
        Context {
            root,
            home: Some(Path::new("/home/dev")),
        },
    )
}

fn command(mapped: Mapped) -> CommandAction {
    match mapped {
        Mapped::Step(StepAction::Command(command)) => command,
        other => panic!("expected a command, got {other:?}"),
    }
}

fn unsupported(mapped: Mapped) -> String {
    match mapped {
        Mapped::Unsupported(reason) => reason,
        other => panic!("expected an unsupported configuration, got {other:?}"),
    }
}

fn gradle_xml(project_path: &str, tasks: &[&str], extra: &str) -> String {
    let tasks = tasks
        .iter()
        .map(|task| format!(r#"<option value="{task}" />"#))
        .collect::<String>();
    format!(
        r#"<configuration name="build" type="GradleRunConfiguration" factoryName="Gradle">
  <ExternalSystemSettings>
    <option name="executionName" />
    <option name="externalProjectPath" value="{project_path}" />
    <option name="externalSystemIdString" value="GRADLE" />
    <option name="taskDescriptions"><list /></option>
    <option name="taskNames"><list>{tasks}</list></option>
    {extra}
  </ExternalSystemSettings>
  <method v="2" />
</configuration>"#
    )
}

#[test]
fn gradle_tasks_and_parameters_run_with_the_wrapper() {
    let root = project(&[]);
    let mapped = map(
        root.path(),
        &gradle_xml(
            "$PROJECT_DIR$",
            &[
                "clean",
                ":app:test",
                "--tests &quot;com.example.FooTest&quot;",
            ],
            r#"<option name="scriptParameters" value="--info --offline" />
               <option name="vmOptions" value="-Xmx2g -Dci=true" />"#,
        ),
    );
    assert_eq!(
        command(mapped),
        CommandAction {
            command: r#"./gradlew clean :app:test --tests "com.example.FooTest" --info --offline -Dci=true"#
                .to_owned(),
            target: CommandTarget::Current,
            cwd: None,
            env: BTreeMap::new(),
        }
    );
}

#[test]
fn gradle_in_a_module_dir_uses_the_root_wrapper_with_project_dir() {
    let root = project(&["app"]);
    let mapped = map(
        root.path(),
        &gradle_xml(
            "$PROJECT_DIR$/app",
            &["assembleDebug"],
            r#"<option name="env"><map><entry key="JAVA_HOME" value="$USER_HOME$/jdk" /></map></option>"#,
        ),
    );
    let command = command(mapped);
    assert_eq!(command.command, "./gradlew -p app assembleDebug");
    assert_eq!(command.cwd, None);
    assert_eq!(
        command.env,
        BTreeMap::from([("JAVA_HOME".to_owned(), "/home/dev/jdk".to_owned())])
    );
}

#[test]
fn gradle_without_a_wrapper_runs_gradle_in_the_project_dir() {
    let root = project(&["app"]);
    fs::remove_file(root.path().join("gradlew")).unwrap();
    let command = command(map(
        root.path(),
        &gradle_xml("$PROJECT_DIR$/app", &["build"], ""),
    ));
    assert_eq!(command.command, "gradle build");
    assert_eq!(command.cwd, Some("app".to_owned()));
}

#[test]
fn gradle_with_an_invalid_env_name_is_unsupported() {
    let root = project(&[]);
    let reason = unsupported(map(
        root.path(),
        &gradle_xml(
            "$PROJECT_DIR$",
            &["build"],
            r#"<option name="env"><map><entry key="A;B" value="x" /></map></option>"#,
        ),
    ));
    assert!(reason.contains("A;B"), "{reason}");
}

#[test]
fn gradle_without_settings_is_unsupported() {
    let root = project(&[]);
    unsupported(map(
        root.path(),
        r#"<configuration name="x" type="GradleRunConfiguration" />"#,
    ));
}

#[test]
fn shell_script_file_runs_with_interpreter_options_and_env() {
    let root = project(&[]);
    let command = command(map(
        root.path(),
        r#"<configuration name="leaks" type="ShConfigurationType">
    <option name="SCRIPT_TEXT" value="" />
    <option name="INDEPENDENT_SCRIPT_PATH" value="true" />
    <option name="SCRIPT_PATH" value="$PROJECT_DIR$/scripts/check leaks.sh" />
    <option name="SCRIPT_OPTIONS" value="--fast" />
    <option name="INDEPENDENT_SCRIPT_WORKING_DIRECTORY" value="true" />
    <option name="SCRIPT_WORKING_DIRECTORY" value="$PROJECT_DIR$/app" />
    <option name="INDEPENDENT_INTERPRETER_PATH" value="true" />
    <option name="INTERPRETER_PATH" value="/bin/zsh" />
    <option name="INTERPRETER_OPTIONS" value="-e" />
    <option name="EXECUTE_IN_TERMINAL" value="true" />
    <option name="EXECUTE_SCRIPT_FILE" value="true" />
    <envs><env name="MODE" value="ci" /></envs>
    <method v="2" />
  </configuration>"#,
    ));
    let script = root.path().join("scripts/check leaks.sh");
    assert_eq!(
        command.command,
        format!("/bin/zsh -e '{}' --fast", script.display())
    );
    assert_eq!(command.cwd, Some("app".to_owned()));
    assert_eq!(
        command.env,
        BTreeMap::from([("MODE".to_owned(), "ci".to_owned())])
    );
}

#[test]
fn shell_script_without_interpreter_runs_the_file_from_its_folder() {
    let root = project(&[]);
    let command = command(map(
        root.path(),
        r#"<configuration name="deploy" type="ShConfigurationType">
    <option name="SCRIPT_PATH" value="tools/deploy.sh" />
    <option name="INTERPRETER_PATH" value="" />
  </configuration>"#,
    ));
    assert_eq!(
        command.command,
        root.path().join("tools/deploy.sh").display().to_string()
    );
    assert_eq!(command.cwd, Some("tools".to_owned()));
}

#[test]
fn shell_script_text_runs_as_written() {
    let root = project(&[]);
    let command = command(map(
        root.path(),
        r#"<configuration name="hello" type="ShConfigurationType">
    <option name="SCRIPT_TEXT" value="echo &quot;${HOME}&quot;&#10;ls" />
    <option name="EXECUTE_SCRIPT_FILE" value="false" />
  </configuration>"#,
    ));
    assert_eq!(command.command, "echo \"${HOME}\"\nls");
    assert_eq!(command.cwd, None);
}

#[test]
fn shell_script_without_a_script_is_unsupported() {
    let root = project(&[]);
    unsupported(map(
        root.path(),
        r#"<configuration name="x" type="ShConfigurationType"><option name="EXECUTE_SCRIPT_FILE" value="false" /></configuration>"#,
    ));
    unsupported(map(
        root.path(),
        r#"<configuration name="x" type="ShConfigurationType" />"#,
    ));
}

fn android_xml(module: &str, options: &str) -> String {
    format!(
        r#"<configuration name="app" type="AndroidRunConfigurationType" factoryName="Android App">
    <module name="{module}" />
    <option name="ANDROID_RUN_CONFIGURATION_SCHEMA_VERSION" value="1" />
    <option name="DEPLOY" value="true" />
    <option name="DEPLOY_APK_FROM_BUNDLE" value="false" />
    {options}
    <option name="ACTIVITY" value="" />
    <option name="SEARCH_ACTIVITY_IN_GLOBAL_SCOPE" value="false" />
  </configuration>"#
    )
}

fn android(mapped: Mapped) -> AndroidRun {
    match mapped {
        Mapped::Step(StepAction::AndroidApp(run)) => run,
        other => panic!("expected an Android run, got {other:?}"),
    }
}

#[test]
fn android_app_with_default_activity() {
    let root = project(&["app"]);
    let run = android(map(
        root.path(),
        &android_xml(
            "MyApp.app.main",
            r#"<option name="MODE" value="default_activity" />
               <option name="ACTIVITY_CLASS" value="com.example.Ignored" />"#,
        ),
    ));
    assert_eq!(
        run,
        AndroidRun {
            gradle_path: ":app".to_owned(),
            launch: AndroidLaunch::DefaultActivity,
        }
    );
}

#[test]
fn android_app_launch_options() {
    let root = project(&["app"]);
    let launch =
        |options: &str| android(map(root.path(), &android_xml("MyApp.app", options))).launch;
    assert_eq!(
        launch(
            r#"<option name="MODE" value="specific_activity" />
               <option name="ACTIVITY_CLASS" value="com.example.debug.MenuActivity" />"#
        ),
        AndroidLaunch::Activity("com.example.debug.MenuActivity".to_owned())
    );
    assert_eq!(
        launch(
            r#"<option name="MODE" value="deep_link" /><option name="DEEP_LINK" value="app://home" />"#
        ),
        AndroidLaunch::DeepLink("app://home".to_owned())
    );
    assert_eq!(
        launch(r#"<option name="MODE" value="do_nothing" />"#),
        AndroidLaunch::Nothing
    );
    // Older configurations have no MODE.
    assert_eq!(launch(""), AndroidLaunch::DefaultActivity);
}

#[test]
fn android_app_problems_are_unsupported() {
    let root = project(&["app"]);
    let reason =
        |module: &str, options: &str| unsupported(map(root.path(), &android_xml(module, options)));
    assert!(reason("MyApp.feature", "").contains("MyApp.feature"));
    reason(
        "MyApp.app",
        r#"<option name="MODE" value="specific_activity" />"#,
    );
    reason("MyApp.app", r#"<option name="MODE" value="other" />"#);
    unsupported(map(
        root.path(),
        r#"<configuration name="a" type="AndroidRunConfigurationType">
    <module name="MyApp.app" /><option name="DEPLOY" value="false" /></configuration>"#,
    ));
    unsupported(map(
        root.path(),
        r#"<configuration name="a" type="AndroidRunConfigurationType"><option name="DEPLOY" value="true" /></configuration>"#,
    ));
}

#[test]
fn junit_class_and_method_run_the_module_test_task() {
    let root = project(&["core/data"]);
    let class = command(map(
        root.path(),
        r#"<configuration name="RepoTest" type="JUnit" factoryName="JUnit">
    <module name="MyApp.core.data.test" />
    <option name="PACKAGE_NAME" value="com.example.data" />
    <option name="MAIN_CLASS_NAME" value="com.example.data.RepoTest" />
    <option name="METHOD_NAME" value="" />
    <option name="TEST_OBJECT" value="class" />
  </configuration>"#,
    ));
    assert_eq!(
        class.command,
        "./gradlew :core:data:test --tests com.example.data.RepoTest"
    );
    let method = command(map(
        root.path(),
        r#"<configuration name="RepoTest.loads" type="JUnit">
    <module name="MyApp.core.data.test" />
    <option name="MAIN_CLASS_NAME" value="com.example.data.RepoTest" />
    <option name="METHOD_NAME" value="loads cached items" />
    <option name="TEST_OBJECT" value="method" />
  </configuration>"#,
    ));
    assert_eq!(
        method.command,
        "./gradlew :core:data:test --tests 'com.example.data.RepoTest.loads cached items'"
    );
}

#[test]
fn junit_package_and_source_set_tasks() {
    let root = project(&["shared", "app"]);
    let package = command(map(
        root.path(),
        r#"<configuration name="all" type="JUnit">
    <module name="MyApp.shared.jvmTest" />
    <option name="PACKAGE_NAME" value="com.example" />
    <option name="TEST_OBJECT" value="package" />
  </configuration>"#,
    ));
    assert_eq!(
        package.command,
        "./gradlew :shared:jvmTest --tests 'com.example.*'"
    );
    let android = command(map(
        root.path(),
        r#"<configuration name="VmTest" type="AndroidJUnit" factoryName="Android JUnit">
    <module name="MyApp.app.unitTest" />
    <option name="MAIN_CLASS_NAME" value="com.example.VmTest" />
    <option name="TEST_OBJECT" value="class" />
  </configuration>"#,
    ));
    assert_eq!(
        android.command,
        "./gradlew :app:testDebugUnitTest --tests com.example.VmTest"
    );
    let android_plain = command(map(
        root.path(),
        r#"<configuration name="VmTest" type="AndroidJUnit">
    <module name="app" />
    <option name="MAIN_CLASS_NAME" value="com.example.VmTest" />
  </configuration>"#,
    ));
    assert_eq!(
        android_plain.command,
        "./gradlew :app:testDebugUnitTest --tests com.example.VmTest"
    );
}

#[test]
fn junit_shapes_without_a_gradle_mapping_are_unsupported() {
    let root = project(&["shared"]);
    unsupported(map(
        root.path(),
        r#"<configuration name="dir" type="JUnit">
    <module name="MyApp.shared.test" /><option name="TEST_OBJECT" value="directory" /></configuration>"#,
    ));
    unsupported(map(
        root.path(),
        r#"<configuration name="common" type="JUnit">
    <module name="MyApp.shared.commonTest" /><option name="MAIN_CLASS_NAME" value="a.B" /></configuration>"#,
    ));
    unsupported(map(
        root.path(),
        r#"<configuration name="nomodule" type="JUnit"><option name="MAIN_CLASS_NAME" value="a.B" /></configuration>"#,
    ));
}

#[test]
fn instrumented_tests_run_connected_android_test() {
    let root = project(&["app"]);
    let xml = |testing_type: &str| {
        format!(
            r#"<configuration name="ui" type="AndroidTestRunConfigurationType">
    <module name="MyApp.app.androidTest" />
    <option name="TESTING_TYPE" value="{testing_type}" />
    <option name="METHOD_NAME" value="opens" />
    <option name="CLASS_NAME" value="com.example.UiTest" />
    <option name="PACKAGE_NAME" value="com.example" />
  </configuration>"#
        )
    };
    let run = |testing_type: &str| command(map(root.path(), &xml(testing_type))).command;
    assert_eq!(run("0"), "./gradlew :app:connectedDebugAndroidTest");
    assert_eq!(
        run("1"),
        "./gradlew :app:connectedDebugAndroidTest -Pandroid.testInstrumentationRunnerArguments.package=com.example"
    );
    assert_eq!(
        run("2"),
        "./gradlew :app:connectedDebugAndroidTest -Pandroid.testInstrumentationRunnerArguments.class=com.example.UiTest"
    );
    assert_eq!(
        run("3"),
        "./gradlew :app:connectedDebugAndroidTest '-Pandroid.testInstrumentationRunnerArguments.class=com.example.UiTest#opens'"
    );
}

#[test]
fn kotlin_application_and_unknown_types_are_unsupported() {
    let root = project(&[]);
    for type_id in [
        "JetRunConfigurationType",
        "Application",
        "KotlinStandaloneScriptRunConfigurationType",
        "PythonConfigurationType",
    ] {
        let reason = unsupported(map(
            root.path(),
            &format!(r#"<configuration name="x" type="{type_id}" />"#),
        ));
        assert!(!reason.is_empty());
    }
}

#[test]
fn compound_lists_its_members() {
    let root = project(&[]);
    assert_eq!(
        map(
            root.path(),
            r#"<configuration name="all" type="CompoundRunConfigurationType">
    <toRun name="build" type="GradleRunConfiguration" />
    <toRun name="app" type="AndroidRunConfigurationType" />
    <method v="2" />
  </configuration>"#,
        ),
        Mapped::Compound(vec![
            ("GradleRunConfiguration".to_owned(), "build".to_owned()),
            ("AndroidRunConfigurationType".to_owned(), "app".to_owned()),
        ])
    );
}
