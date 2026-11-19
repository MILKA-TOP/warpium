use std::fs;
use std::path::Path;

use super::*;

fn project(dirs: &[&str]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("settings.gradle.kts"), "").unwrap();
    fs::write(root.path().join("build.gradle.kts"), "").unwrap();
    for dir in dirs {
        let dir = root.path().join(dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("build.gradle.kts"), "").unwrap();
    }
    root
}

fn gradle_path(root: &Path, name: &str) -> Option<(String, Option<String>)> {
    resolve_module(root, name).map(|module| (module.gradle_path, module.source_set))
}

#[test]
fn module_names_with_root_project_name_and_source_set_resolve() {
    let root = project(&["app", "core/data"]);
    let root = root.path();
    assert_eq!(
        gradle_path(root, "MyApp.app.main"),
        Some((":app".to_owned(), Some("main".to_owned())))
    );
    assert_eq!(
        gradle_path(root, "MyApp.core.data.unitTest"),
        Some((":core:data".to_owned(), Some("unitTest".to_owned())))
    );
    assert_eq!(
        gradle_path(root, "MyApp.app"),
        Some((":app".to_owned(), None))
    );
    assert_eq!(gradle_path(root, "app"), Some((":app".to_owned(), None)));
}

#[test]
fn root_project_names_with_dots_are_dropped() {
    let root = project(&["app-store"]);
    assert_eq!(
        gradle_path(root.path(), "My Project.My_Project.app-store"),
        Some((":app-store".to_owned(), None))
    );
}

#[test]
fn a_module_named_like_a_source_set_matches_as_written_first() {
    let root = project(&["core/test", "core"]);
    assert_eq!(
        gradle_path(root.path(), "MyApp.core.test"),
        Some((":core:test".to_owned(), None))
    );
    assert_eq!(
        gradle_path(root.path(), "MyApp.core.test.main"),
        Some((":core:test".to_owned(), Some("main".to_owned())))
    );
}

#[test]
fn the_root_project_resolves_by_its_name() {
    let root = project(&[]);
    assert_eq!(
        gradle_path(root.path(), "MyApp.test"),
        Some((String::new(), Some("test".to_owned())))
    );
    assert_eq!(
        gradle_path(root.path(), "MyApp"),
        Some((String::new(), None))
    );
}

#[test]
fn unknown_modules_dont_resolve() {
    let root = project(&["app"]);
    assert_eq!(gradle_path(root.path(), "MyApp.feature.main"), None);
    assert_eq!(gradle_path(root.path(), "MyApp.feature.app2"), None);
    assert_eq!(gradle_path(root.path(), ""), None);
}

#[test]
fn task_paths_include_the_project() {
    let root = project(&["app"]);
    let module = resolve_module(root.path(), "app").unwrap();
    assert_eq!(module.task("test"), ":app:test");
    let root_module = resolve_module(root.path(), "MyApp").unwrap();
    assert_eq!(root_module.task("test"), ":test");
}

#[test]
fn the_wrapper_is_found_above_the_project_dir() {
    let root = project(&["tools/build-logic"]);
    fs::write(root.path().join("gradlew"), "").unwrap();
    let launcher = gradle_launcher(root.path(), &root.path().join("tools/build-logic"));
    assert_eq!(
        launcher,
        GradleLauncher {
            program: "./gradlew",
            cwd: root.path().to_path_buf(),
            project_arg: Some("tools/build-logic".to_owned()),
        }
    );
    assert_eq!(gradle_launcher(root.path(), root.path()).project_arg, None);
}

#[test]
fn without_a_wrapper_gradle_runs_in_the_project_dir() {
    let root = project(&["app"]);
    let launcher = gradle_launcher(root.path(), &root.path().join("app"));
    assert_eq!(launcher.program, "gradle");
    assert_eq!(launcher.cwd, root.path().join("app"));
    assert_eq!(launcher.project_arg, None);
}
