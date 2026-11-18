use std::path::{Path, PathBuf};

use super::*;

fn p(path: &str) -> PathBuf {
    PathBuf::from(path)
}

fn roots(paths: &[&str]) -> LockedRoots {
    LockedRoots::new(paths.iter().map(|path| p(path))).expect("at least one root")
}

// Mode and roots.

#[test]
fn locking_to_no_folders_follows_terminals() {
    assert_eq!(
        ProjectRoot::locked(Vec::<PathBuf>::new()),
        ProjectRoot::Follow
    );
}

#[test]
fn locked_roots_drop_duplicates_and_empty_paths_keeping_order() {
    let project = ProjectRoot::locked([p("/a"), p(""), p("/b"), p("/a")]);

    assert_eq!(project.roots(), &[p("/a"), p("/b")]);
    assert_eq!(project.primary(), Some(Path::new("/a")));
}

#[test]
fn following_project_has_no_roots() {
    assert_eq!(ProjectRoot::Follow.roots(), &[] as &[PathBuf]);
    assert_eq!(ProjectRoot::Follow.primary(), None);
}

#[test]
fn adding_a_folder_to_a_following_project_locks_it_to_that_folder() {
    let project = ProjectRoot::Follow.with_root_added(p("/a"));

    assert_eq!(project, ProjectRoot::locked_to(p("/a")));
}

#[test]
fn adding_a_folder_appends_it_after_the_primary_root() {
    let project = ProjectRoot::locked_to(p("/a")).with_root_added(p("/b"));

    assert_eq!(project.roots(), &[p("/a"), p("/b")]);
}

#[test]
fn adding_a_folder_already_in_the_project_changes_nothing() {
    let project = ProjectRoot::locked([p("/a"), p("/b")]).with_root_added(p("/a"));

    assert_eq!(project.roots(), &[p("/a"), p("/b")]);
}

#[test]
fn removing_the_primary_root_promotes_the_next_one() {
    let project = ProjectRoot::locked([p("/a"), p("/b")]).with_root_removed(Path::new("/a"));

    assert_eq!(project, ProjectRoot::locked_to(p("/b")));
}

#[test]
fn removing_the_only_root_keeps_the_project_locked() {
    let project = ProjectRoot::locked_to(p("/a")).with_root_removed(Path::new("/a"));

    assert_eq!(project, ProjectRoot::locked_to(p("/a")));
}

#[test]
fn adding_a_folder_inside_a_root_changes_nothing() {
    let project = ProjectRoot::locked_to(p("/repo")).with_root_added(p("/repo/sub"));

    assert_eq!(project, ProjectRoot::locked_to(p("/repo")));
}

#[test]
fn adding_a_folder_containing_a_root_changes_nothing() {
    let project = ProjectRoot::locked_to(p("/repo/sub")).with_root_added(p("/repo"));

    assert_eq!(project, ProjectRoot::locked_to(p("/repo/sub")));
}

// Terminal placement.

#[test]
fn following_project_never_places_terminals() {
    let placement = ProjectRoot::Follow.place_terminal(&TerminalCwd::Host(p("/elsewhere")));

    assert_eq!(placement, None);
}

#[test]
fn terminal_in_a_subdirectory_of_a_root_is_in_the_project() {
    let project = ProjectRoot::locked([p("/proj/a"), p("/proj/b")]);

    let placement = project.place_terminal(&TerminalCwd::Host(p("/proj/b/src")));

    assert_eq!(placement, Some(TerminalPlacement::InProject));
}

#[test]
fn terminal_in_an_unrelated_host_folder_is_outside_the_project() {
    let project = ProjectRoot::locked_to(p("/proj"));

    let placement = project.place_terminal(&TerminalCwd::Host(p("/projects/other")));

    assert_eq!(placement, Some(TerminalPlacement::OutsideProject));
}

#[test]
fn terminal_in_a_container_or_ssh_session_is_not_on_host() {
    let project = ProjectRoot::locked_to(p("/proj"));

    let placement = project.place_terminal(&TerminalCwd::NotOnHost);

    assert_eq!(placement, Some(TerminalPlacement::NotOnHost));
}

#[test]
fn indicator_is_shown_only_outside_the_project() {
    assert_eq!(TerminalPlacement::InProject.indicator_text(), None);
    assert_eq!(TerminalPlacement::Unknown.indicator_text(), None);
    assert_eq!(
        TerminalPlacement::OutsideProject.indicator_text(),
        Some("Terminal is outside the project")
    );
    assert_eq!(
        TerminalPlacement::NotOnHost.indicator_text(),
        Some("Terminal is outside the project (not a local folder)")
    );
}

#[test]
fn containing_root_prefers_the_deepest_root() {
    let roots = [p("/repo"), p("/repo/app")];

    assert_eq!(
        containing_root(&roots, Path::new("/repo/app/src")),
        Some(Path::new("/repo/app"))
    );
}

#[test]
fn containing_root_compares_whole_components() {
    let roots = [p("/repo")];

    assert_eq!(containing_root(&roots, Path::new("/repository")), None);
}

// Lock target.

#[test]
fn lock_target_is_the_explorer_root_holding_the_terminal() {
    let target = lock_target(&[p("/b"), p("/a")], Some(Path::new("/a/src")));

    assert_eq!(target, Some(p("/a")));
}

#[test]
fn lock_target_is_the_most_recent_explorer_root_when_the_terminal_is_elsewhere() {
    let target = lock_target(&[p("/b"), p("/a")], Some(Path::new("/tmp")));

    assert_eq!(target, Some(p("/b")));
}

#[test]
fn lock_target_is_the_terminal_directory_without_explorer_roots() {
    let target = lock_target(&[], Some(Path::new("/tmp/work")));

    assert_eq!(target, Some(p("/tmp/work")));
}

#[test]
fn lock_target_is_none_without_roots_or_directory() {
    assert_eq!(lock_target(&[], None), None);
}

// Locked layout: which roots to show, and which terminal belongs to each.

#[test]
fn locked_layout_shows_only_the_locked_roots() {
    let terminals = [
        (1, TerminalCwd::Host(p("/elsewhere"))),
        (2, TerminalCwd::NotOnHost),
        (3, TerminalCwd::Unknown),
    ];

    let layout = locked_layout(&roots(&["/proj", "/lib"]), &terminals, Some(1));

    assert_eq!(layout.display_roots, vec![p("/proj"), p("/lib")]);
    assert_eq!(layout.root_terminals, vec![]);
    assert_eq!(layout.focused_root, p("/proj"));
}

#[test]
fn locked_layout_maps_each_root_to_a_terminal_inside_it() {
    let terminals = [
        (1, TerminalCwd::Host(p("/lib/src"))),
        (2, TerminalCwd::Host(p("/proj"))),
    ];

    let layout = locked_layout(&roots(&["/proj", "/lib"]), &terminals, None);

    assert_eq!(layout.root_terminals, vec![(p("/proj"), 2), (p("/lib"), 1)]);
}

#[test]
fn locked_layout_prefers_the_focused_terminal_for_its_root() {
    let terminals = [
        (1, TerminalCwd::Host(p("/proj/a"))),
        (2, TerminalCwd::Host(p("/proj/b"))),
    ];

    let layout = locked_layout(&roots(&["/proj"]), &terminals, Some(2));

    assert_eq!(layout.root_terminals, vec![(p("/proj"), 2)]);
}

#[test]
fn locked_layout_focuses_the_root_of_the_focused_terminal() {
    let terminals = [(1, TerminalCwd::Host(p("/lib/src")))];

    let layout = locked_layout(&roots(&["/proj", "/lib"]), &terminals, Some(1));

    assert_eq!(layout.focused_root, p("/lib"));
}

#[test]
fn locked_layout_maps_a_terminal_to_its_deepest_root() {
    let terminals = [(1, TerminalCwd::Host(p("/repo/app/src")))];

    let layout = locked_layout(&roots(&["/repo", "/repo/app"]), &terminals, None);

    assert_eq!(layout.root_terminals, vec![(p("/repo/app"), 1)]);
}

// Persistence.

#[test]
fn following_project_persists_as_empty_columns() {
    assert_eq!(
        ProjectRoot::Follow.to_persisted(),
        PersistedProjectRoot {
            primary: None,
            extra_roots: None,
        }
    );
}

#[test]
fn single_root_persists_without_extra_roots() {
    assert_eq!(
        ProjectRoot::locked_to(p("/proj")).to_persisted(),
        PersistedProjectRoot {
            primary: Some(p("/proj")),
            extra_roots: None,
        }
    );
}

#[test]
fn extra_roots_persist_as_a_json_array() {
    assert_eq!(
        ProjectRoot::locked([p("/proj"), p("/lib"), p("/docs")]).to_persisted(),
        PersistedProjectRoot {
            primary: Some(p("/proj")),
            extra_roots: Some(r#"["/lib","/docs"]"#.to_owned()),
        }
    );
}

#[test]
fn persisted_project_round_trips() {
    let project = ProjectRoot::locked([p("/proj"), p("/with space/lib")]);

    let persisted = project.to_persisted();
    let restored = ProjectRoot::from_persisted(persisted.primary, persisted.extra_roots.as_deref());

    assert_eq!(restored, project);
}

#[test]
fn row_from_an_older_build_with_a_pinned_root_restores_locked_to_it() {
    let restored = ProjectRoot::from_persisted(Some(p("/repo/app")), None);

    assert_eq!(restored, ProjectRoot::locked_to(p("/repo/app")));
}

#[test]
fn row_without_a_primary_root_follows_terminals() {
    let restored = ProjectRoot::from_persisted(None, Some(r#"["/lib"]"#));

    assert_eq!(restored, ProjectRoot::Follow);
}

#[test]
fn unreadable_extra_roots_are_ignored() {
    let restored = ProjectRoot::from_persisted(Some(p("/proj")), Some("not json"));

    assert_eq!(restored, ProjectRoot::locked_to(p("/proj")));
}
