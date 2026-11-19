use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{FileProjectRoot, ProjectRootKind, RootMarkers, project_root_for_file};

/// Markers held in memory: directories with Gradle settings and repositories.
#[derive(Default)]
struct Markers {
    gradle: HashSet<PathBuf>,
    repositories: HashSet<PathBuf>,
}

impl Markers {
    fn gradle(mut self, dir: &str) -> Self {
        self.gradle.insert(PathBuf::from(dir));
        self
    }

    fn repository(mut self, dir: &str) -> Self {
        self.repositories.insert(PathBuf::from(dir));
        self
    }
}

impl RootMarkers for Markers {
    fn has_gradle_settings(&self, dir: &Path) -> bool {
        self.gradle.contains(dir)
    }

    fn is_repository(&self, dir: &Path) -> bool {
        self.repositories.contains(dir)
    }
}

fn root(path: &str, kind: ProjectRootKind) -> Option<FileProjectRoot> {
    Some(FileProjectRoot {
        root: PathBuf::from(path),
        kind,
    })
}

fn locked(paths: &[&str]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

#[test]
fn plain_locked_folder_is_the_root() {
    let markers = Markers::default();

    let choice = project_root_for_file(
        Path::new("/home/u/kt/src/Main.kt"),
        &locked(&["/home/u/kt"]),
        &markers,
    );

    assert_eq!(choice, root("/home/u/kt", ProjectRootKind::Locked));
}

#[test]
fn file_outside_markers_and_locked_folders_has_no_root() {
    let markers = Markers::default();

    let choice = project_root_for_file(
        Path::new("/home/u/kt/Main.kt"),
        &locked(&["/home/u/other"]),
        &markers,
    );

    assert_eq!(choice, None);
}

#[test]
fn gradle_build_inside_locked_folder_wins() {
    let markers = Markers::default().gradle("/work/app");

    let choice = project_root_for_file(
        Path::new("/work/app/core/src/main/kotlin/A.kt"),
        &locked(&["/work"]),
        &markers,
    );

    assert_eq!(choice, root("/work/app", ProjectRootKind::Gradle));
}

#[test]
fn outermost_gradle_build_wins_over_nested_repository_inside_locked_folder() {
    let markers = Markers::default()
        .gradle("/work/app")
        .gradle("/work/app/libs/sub")
        .repository("/work/app/libs/sub");

    let choice = project_root_for_file(
        Path::new("/work/app/libs/sub/src/A.kt"),
        &locked(&["/work/app"]),
        &markers,
    );

    assert_eq!(choice, root("/work/app", ProjectRootKind::Gradle));
}

#[test]
fn repository_inside_locked_folder_wins_without_gradle_settings() {
    let markers = Markers::default().repository("/work/proj");

    let choice = project_root_for_file(
        Path::new("/work/proj/src/A.kt"),
        &locked(&["/work"]),
        &markers,
    );

    assert_eq!(choice, root("/work/proj", ProjectRootKind::Repository));
}

#[test]
fn markers_above_locked_folder_are_ignored() {
    let markers = Markers::default().gradle("/repo").repository("/repo");

    let choice = project_root_for_file(
        Path::new("/repo/feature/src/A.kt"),
        &locked(&["/repo/feature"]),
        &markers,
    );

    assert_eq!(choice, root("/repo/feature", ProjectRootKind::Locked));
}

#[test]
fn locked_folder_with_gradle_settings_is_a_gradle_root() {
    let markers = Markers::default()
        .gradle("/work/app")
        .repository("/work/app");

    let choice = project_root_for_file(
        Path::new("/work/app/src/A.kt"),
        &locked(&["/work/app"]),
        &markers,
    );

    assert_eq!(choice, root("/work/app", ProjectRootKind::Gradle));
}

#[test]
fn deepest_locked_folder_holding_the_file_is_used() {
    let markers = Markers::default();

    let choice = project_root_for_file(
        Path::new("/work/kt/src/A.kt"),
        &locked(&["/elsewhere", "/work", "/work/kt"]),
        &markers,
    );

    assert_eq!(choice, root("/work/kt", ProjectRootKind::Locked));
}

#[test]
fn locked_folder_is_matched_by_components_not_prefix() {
    let markers = Markers::default();

    let choice = project_root_for_file(
        Path::new("/work/kotlin/A.kt"),
        &locked(&["/work/kt"]),
        &markers,
    );

    assert_eq!(choice, None);
}

#[test]
fn unlocked_file_uses_outermost_gradle_build_inside_its_repository() {
    let markers = Markers::default()
        .gradle("/outer")
        .repository("/outer/repo")
        .gradle("/outer/repo")
        .gradle("/outer/repo/included");

    let choice = project_root_for_file(Path::new("/outer/repo/included/A.kt"), &[], &markers);

    assert_eq!(choice, root("/outer/repo", ProjectRootKind::Gradle));
}

#[test]
fn unlocked_file_uses_repository_without_gradle_settings() {
    let markers = Markers::default().repository("/repo");

    let choice = project_root_for_file(Path::new("/repo/src/A.kt"), &[], &markers);

    assert_eq!(choice, root("/repo", ProjectRootKind::Repository));
}

#[test]
fn unlocked_file_outside_a_repository_uses_outermost_gradle_build() {
    let markers = Markers::default().gradle("/a").gradle("/a/b");

    let choice = project_root_for_file(Path::new("/a/b/src/A.kt"), &[], &markers);

    assert_eq!(choice, root("/a", ProjectRootKind::Gradle));
}
