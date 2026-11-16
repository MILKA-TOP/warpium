use std::fs;
use std::path::{Path, PathBuf};

use repo_metadata::repositories::DetectedRepositories;
use warp_util::host_id::HostId;
use warp_util::remote_path::RemotePath;
use warp_util::standardized_path::StandardizedPath;
use warpium_project_root::LockedRoots;
use warpui::{App, EntityId};

use crate::code::buffer_location::LocalOrRemotePath;
use crate::pane_group::WorkingDirectoriesModel;

fn local(path: &Path) -> LocalOrRemotePath {
    LocalOrRemotePath::Local(path.to_path_buf())
}

fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).expect("canonical path")
}

#[test]
fn locked_project_shows_its_folders_whatever_the_terminals_do() {
    App::test((), |mut app| async move {
        let detected_repos = app.add_singleton_model(|_| DetectedRepositories::default());
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let project = temp_dir.path().join("project");
        let library = temp_dir.path().join("library");
        let elsewhere = temp_dir.path().join("elsewhere");
        for dir in [&project, &library, &elsewhere] {
            fs::create_dir_all(dir).expect("create dir");
        }
        detected_repos.update(&mut app, |repos, _| {
            repos.insert_test_repo_root(
                StandardizedPath::from_local_canonicalized(&canonical(&project))
                    .expect("canonicalized path"),
            );
        });
        let roots = LockedRoots::new([project.clone(), library.clone()]).expect("roots");
        let pane_group_id = EntityId::new();
        let host_terminal = EntityId::new();
        let ssh_terminal = EntityId::new();
        let ssh_cwd = LocalOrRemotePath::Remote(RemotePath::new(
            HostId::new("host".to_owned()),
            StandardizedPath::try_new("/srv/app").expect("remote path"),
        ));

        let model = app.add_model(|_| WorkingDirectoriesModel::new());
        let (directories, repositories) = model.update(&mut app, |model, ctx| {
            model.refresh_locked_project_for_pane_group(
                pane_group_id,
                &roots,
                vec![
                    (host_terminal, local(&canonical(&elsewhere))),
                    (ssh_terminal, ssh_cwd),
                ],
                Some(host_terminal),
                ctx,
            );
            (
                model
                    .most_recent_directories_for_pane_group(pane_group_id)
                    .expect("pane group exists")
                    .map(|directory| (directory.path, directory.terminal_id))
                    .collect::<Vec<_>>(),
                model
                    .most_recent_repositories_for_pane_group(pane_group_id)
                    .expect("pane group exists")
                    .collect::<Vec<_>>(),
            )
        });

        assert_eq!(
            directories,
            vec![
                (local(&canonical(&project)), None),
                (local(&canonical(&library)), None),
            ],
            "terminals outside the project neither add roots nor attach to them"
        );
        assert_eq!(repositories, vec![local(&canonical(&project))]);
    });
}

#[test]
fn locked_project_keeps_its_folders_without_any_terminal_directory() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| DetectedRepositories::default());
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let project = temp_dir.path().join("project");
        fs::create_dir_all(&project).expect("create dir");
        let roots = LockedRoots::new([project.clone()]).expect("roots");
        let pane_group_id = EntityId::new();

        let model = app.add_model(|_| WorkingDirectoriesModel::new());
        let directories = model.update(&mut app, |model, ctx| {
            // A terminal inside a container reports a path that doesn't exist here, so it has no
            // working directory at all.
            model.refresh_locked_project_for_pane_group(pane_group_id, &roots, vec![], None, ctx);
            model
                .most_recent_directories_for_pane_group(pane_group_id)
                .expect("pane group exists")
                .map(|directory| directory.path)
                .collect::<Vec<_>>()
        });

        assert_eq!(directories, vec![local(&canonical(&project))]);
    });
}

#[test]
fn locked_project_attaches_terminals_inside_it_to_their_root_and_repository() {
    App::test((), |mut app| async move {
        let detected_repos = app.add_singleton_model(|_| DetectedRepositories::default());
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let repo = temp_dir.path().join("repo");
        let app_dir = repo.join("app");
        let src = app_dir.join("src");
        fs::create_dir_all(&src).expect("create dir");
        detected_repos.update(&mut app, |repos, _| {
            repos.insert_test_repo_root(
                StandardizedPath::from_local_canonicalized(&canonical(&repo))
                    .expect("canonicalized path"),
            );
        });
        let roots = LockedRoots::new([app_dir.clone()]).expect("roots");
        let pane_group_id = EntityId::new();
        let terminal = EntityId::new();

        let model = app.add_model(|_| WorkingDirectoriesModel::new());
        let (root_terminal, repo_terminal) = model.update(&mut app, |model, ctx| {
            model.refresh_locked_project_for_pane_group(
                pane_group_id,
                &roots,
                vec![(terminal, local(&canonical(&src)))],
                Some(terminal),
                ctx,
            );
            (
                model.get_terminal_id_for_root_path(pane_group_id, &local(&canonical(&app_dir))),
                model.get_terminal_id_for_root_path(pane_group_id, &local(&canonical(&repo))),
            )
        });

        assert_eq!(root_terminal, Some(terminal));
        assert_eq!(repo_terminal, Some(terminal));
    });
}
