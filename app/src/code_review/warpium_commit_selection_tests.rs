use super::*;

fn paths(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| path.to_string()).collect()
}

#[test]
fn files_start_included() {
    let selection = CommitSelection::default();
    assert!(selection.is_included("a.txt"));
    assert_eq!(selection.commit_subset(&paths(&["a.txt"])), None);
}

#[test]
fn toggle_excludes_and_reincludes() {
    let mut selection = CommitSelection::default();
    selection.toggle("a.txt");
    assert!(!selection.is_included("a.txt"));
    selection.toggle("a.txt");
    assert!(selection.is_included("a.txt"));
}

#[test]
fn subset_lists_included_files_in_order() {
    let mut selection = CommitSelection::default();
    selection.toggle("b.txt");
    assert_eq!(
        selection.commit_subset(&paths(&["a.txt", "b.txt", "c.txt"])),
        Some(paths(&["a.txt", "c.txt"]))
    );
}

#[test]
fn stale_exclusions_do_not_make_a_subset() {
    let mut selection = CommitSelection::default();
    selection.toggle("gone.txt");
    assert_eq!(selection.commit_subset(&paths(&["a.txt"])), None);
}

#[test]
fn excluding_everything_yields_an_empty_subset() {
    let mut selection = CommitSelection::default();
    selection.set_included(&paths(&["a.txt", "b.txt"]), false);
    assert_eq!(
        selection.commit_subset(&paths(&["a.txt", "b.txt"])),
        Some(Vec::new())
    );
}

#[test]
fn group_state_is_tri_state() {
    let mut selection = CommitSelection::default();
    let group = paths(&["src/a.rs", "src/b.rs"]);
    assert_eq!(selection.group_state(&group), GroupCheckState::All);
    selection.toggle("src/a.rs");
    assert_eq!(selection.group_state(&group), GroupCheckState::Some);
    selection.toggle("src/b.rs");
    assert_eq!(selection.group_state(&group), GroupCheckState::None);
}

#[test]
fn set_included_updates_only_the_given_paths() {
    let mut selection = CommitSelection::default();
    selection.set_included(&paths(&["a.txt", "b.txt"]), false);
    selection.set_included(&paths(&["a.txt"]), true);
    assert!(selection.is_included("a.txt"));
    assert!(!selection.is_included("b.txt"));
}
