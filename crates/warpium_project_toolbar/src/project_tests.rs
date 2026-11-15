use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;

fn mkdirs(base: &Path, relative: &str) -> PathBuf {
    let path = base.join(relative);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn pinned_explorer_root_wins() {
    let temp = TempDir::new().unwrap();
    let pinned = mkdirs(temp.path(), "pinned");
    let repo = mkdirs(temp.path(), "repo");
    mkdirs(&repo, ".git");
    mkdirs(&repo, ".warpium");

    let project = detect_project(Some(&pinned), Some(&repo));

    assert_eq!(
        project,
        Some(ProjectContext {
            root: pinned,
            source: ProjectSource::PinnedExplorerRoot,
        })
    );
}

#[test]
fn missing_pinned_root_falls_through_to_the_cwd() {
    let temp = TempDir::new().unwrap();
    let repo = mkdirs(temp.path(), "repo");
    mkdirs(&repo, ".git");

    let project = detect_project(Some(&temp.path().join("gone")), Some(&repo));

    assert_eq!(project.map(|project| project.source), Some(ProjectSource::GitRepository));
}

#[test]
fn nearest_warpium_directory_wins_over_the_git_root() {
    let temp = TempDir::new().unwrap();
    let repo = mkdirs(temp.path(), "repo");
    mkdirs(&repo, ".git");
    mkdirs(&repo, ".warpium");
    let module = mkdirs(&repo, "android");
    mkdirs(&module, ".warpium");
    let cwd = mkdirs(&module, "app/src");

    let project = detect_project(None, Some(&cwd));

    assert_eq!(
        project,
        Some(ProjectContext {
            root: module,
            source: ProjectSource::WarpiumDirectory,
        })
    );
}

#[test]
fn warpium_directory_above_the_git_root_is_found() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();
    mkdirs(&workspace, ".warpium");
    let repo = mkdirs(&workspace, "repo");
    mkdirs(&repo, ".git");

    let project = detect_project(None, Some(&repo));

    assert_eq!(
        project,
        Some(ProjectContext {
            root: workspace,
            source: ProjectSource::WarpiumDirectory,
        })
    );
}

#[test]
fn a_warpium_file_is_not_a_project_marker() {
    let temp = TempDir::new().unwrap();
    let repo = mkdirs(temp.path(), "repo");
    mkdirs(&repo, ".git");
    let cwd = mkdirs(&repo, "sub");
    fs::write(cwd.join(".warpium"), "").unwrap();

    let project = detect_project(None, Some(&cwd));

    assert_eq!(
        project,
        Some(ProjectContext {
            root: repo,
            source: ProjectSource::GitRepository,
        })
    );
}

#[test]
fn git_worktree_with_a_git_file_is_a_repository() {
    let temp = TempDir::new().unwrap();
    let worktree = mkdirs(temp.path(), "worktree");
    fs::write(worktree.join(".git"), "gitdir: /elsewhere").unwrap();
    let cwd = mkdirs(&worktree, "src");

    let project = detect_project(None, Some(&cwd));

    assert_eq!(
        project,
        Some(ProjectContext {
            root: worktree,
            source: ProjectSource::GitRepository,
        })
    );
}

#[test]
fn no_project_outside_any_marker() {
    let temp = TempDir::new().unwrap();
    let cwd = mkdirs(temp.path(), "plain/dir");

    assert_eq!(detect_project(None, Some(&cwd)), None);
    assert_eq!(detect_project(None, None), None);
}

#[test]
fn project_id_is_readable_and_stable() {
    let id = project_id(Path::new("/Users/me/kmp-bspb-demo"));

    assert!(id.starts_with("kmp-bspb-demo-"), "id: {id}");
    assert_eq!(id.len(), "kmp-bspb-demo-".len() + 12);
    assert_eq!(id, project_id(Path::new("/Users/me/kmp-bspb-demo")));
}

#[test]
fn project_ids_differ_for_same_named_folders() {
    assert_ne!(
        project_id(Path::new("/a/app")),
        project_id(Path::new("/b/app"))
    );
}

#[test]
fn project_id_replaces_unsafe_characters() {
    let id = project_id(Path::new("/work/My Project: v2"));

    assert!(id.starts_with("My-Project--v2-"), "id: {id}");
}

#[test]
fn project_id_of_the_filesystem_root_has_a_fallback_name() {
    assert!(project_id(Path::new("/")).starts_with("project-"));
}

#[test]
fn config_paths() {
    let root = Path::new("/work/app");

    assert_eq!(
        team_config_path(root),
        PathBuf::from("/work/app/.warpium/toolbar.toml")
    );
    assert_eq!(
        personal_config_path(Path::new("/home/me/.warp-oss"), root),
        PathBuf::from(format!(
            "/home/me/.warp-oss/projects/{}/toolbar.toml",
            project_id(root)
        ))
    );
}
