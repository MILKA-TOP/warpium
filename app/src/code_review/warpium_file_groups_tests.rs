use super::*;

fn files(paths: &[(&str, bool)]) -> Vec<(String, bool)> {
    paths
        .iter()
        .map(|(path, untracked)| ((*path).to_owned(), *untracked))
        .collect()
}

#[test]
fn flat_lists_tracked_then_unversioned_node() {
    let rows = build_rows(
        &files(&[("a.rs", false), ("dir/b.rs", false), ("new.txt", true)]),
        GroupBy::Flat,
        &Default::default(),
        true,
        true,
    );
    assert_eq!(
        rows,
        vec![
            FileRow::File {
                path: "a.rs".to_owned(),
                depth: 0
            },
            FileRow::File {
                path: "dir/b.rs".to_owned(),
                depth: 0
            },
            FileRow::UnversionedNode {
                file_count: 1,
                expanded: true
            },
            FileRow::File {
                path: "new.txt".to_owned(),
                depth: 0
            },
        ]
    );
}

#[test]
fn flat_collapsed_unversioned_node_hides_untracked_files() {
    let rows = build_rows(
        &files(&[("a.rs", false), ("new.txt", true)]),
        GroupBy::Flat,
        &Default::default(),
        true,
        false,
    );
    assert_eq!(
        rows,
        vec![
            FileRow::File {
                path: "a.rs".to_owned(),
                depth: 0
            },
            FileRow::UnversionedNode {
                file_count: 1,
                expanded: false
            },
        ]
    );
}

#[test]
fn directory_groups_in_tree_order_with_joined_chains() {
    let rows = build_rows(
        &files(&[
            ("src/main/kotlin/App.kt", false),
            ("src/main/kotlin/Util.kt", false),
            ("src/test/Test.kt", false),
            ("README.md", false),
        ]),
        GroupBy::Directory,
        &Default::default(),
        true,
        false,
    );
    // `src` keeps its own row because it holds more than one child; the
    // single-child chain below it joins into `main/kotlin` (Git Log pane
    // semantics), keyed by its full repo-relative path.
    assert_eq!(
        rows,
        vec![
            FileRow::Directory {
                path: "src".to_owned(),
                label: "src".to_owned(),
                depth: 0,
                file_count: 3,
                expanded: true,
            },
            FileRow::Directory {
                path: "src/main/kotlin".to_owned(),
                label: "main/kotlin".to_owned(),
                depth: 1,
                file_count: 2,
                expanded: true,
            },
            FileRow::File {
                path: "src/main/kotlin/App.kt".to_owned(),
                depth: 2
            },
            FileRow::File {
                path: "src/main/kotlin/Util.kt".to_owned(),
                depth: 2
            },
            FileRow::Directory {
                path: "src/test".to_owned(),
                label: "test".to_owned(),
                depth: 1,
                file_count: 1,
                expanded: true,
            },
            FileRow::File {
                path: "src/test/Test.kt".to_owned(),
                depth: 2
            },
            FileRow::File {
                path: "README.md".to_owned(),
                depth: 0
            },
        ]
    );
}

#[test]
fn collapsed_directory_skips_its_subtree() {
    let collapsed: std::collections::HashSet<String> =
        ["src/main/kotlin".to_owned()].into_iter().collect();
    let rows = build_rows(
        &files(&[
            ("src/main/kotlin/App.kt", false),
            ("src/main/kotlin/Util.kt", false),
            ("src/readme.txt", false),
        ]),
        GroupBy::Directory,
        &collapsed,
        true,
        false,
    );
    // Collapse keys are full repo-relative paths; the collapsed node's
    // subtree (App.kt, Util.kt) is skipped, its sibling file survives.
    assert_eq!(
        rows,
        vec![
            FileRow::Directory {
                path: "src".to_owned(),
                label: "src".to_owned(),
                depth: 0,
                file_count: 3,
                expanded: true,
            },
            FileRow::Directory {
                path: "src/main/kotlin".to_owned(),
                label: "main/kotlin".to_owned(),
                depth: 1,
                file_count: 2,
                expanded: false,
            },
            FileRow::File {
                path: "src/readme.txt".to_owned(),
                depth: 1
            },
        ]
    );
}

#[test]
fn repository_mode_roots_the_flat_list_under_the_repo_node() {
    let rows = build_rows(
        &files(&[("a.rs", false), ("b.rs", false), ("new.txt", true)]),
        GroupBy::Repository,
        &Default::default(),
        true,
        true,
    );
    assert_eq!(
        rows,
        vec![
            FileRow::Repository {
                name: String::new(),
                file_count: 2,
                expanded: true,
            },
            FileRow::File {
                path: "a.rs".to_owned(),
                depth: 1
            },
            FileRow::File {
                path: "b.rs".to_owned(),
                depth: 1
            },
            FileRow::UnversionedNode {
                file_count: 1,
                expanded: true
            },
            FileRow::File {
                path: "new.txt".to_owned(),
                depth: 1
            },
        ]
    );
}

#[test]
fn collapsed_repository_node_hides_everything_but_itself_and_unversioned() {
    let rows = build_rows(
        &files(&[("a.rs", false), ("new.txt", true)]),
        GroupBy::Repository,
        &Default::default(),
        false,
        false,
    );
    assert_eq!(
        rows,
        vec![
            FileRow::Repository {
                name: String::new(),
                file_count: 1,
                expanded: false,
            },
            FileRow::UnversionedNode {
                file_count: 1,
                expanded: false
            },
        ]
    );
}

#[test]
fn untracked_files_never_appear_in_directory_nodes() {
    let rows = build_rows(
        &files(&[("src/a.rs", false), ("src/generated.txt", true)]),
        GroupBy::Directory,
        &Default::default(),
        true,
        true,
    );
    // The src node counts only the tracked file, and the untracked file sits
    // at depth 0 under the Unversioned Files node, not inside src.
    assert!(rows.iter().any(|row| matches!(
        row,
        FileRow::Directory { path, file_count: 1, .. } if path == "src"
    )));
    assert!(rows.iter().any(|row| matches!(
        row,
        FileRow::File { path, depth: 0 } if path == "src/generated.txt"
    )));
    assert!(rows.iter().any(|row| matches!(
        row,
        FileRow::File { path, depth: 1 } if path == "src/a.rs"
    )));
}

#[test]
fn group_by_settings_round_trip() {
    for group_by in GroupBy::all() {
        assert_eq!(
            GroupBy::from_settings(group_by.as_settings_value()),
            group_by
        );
    }
    assert_eq!(GroupBy::from_settings("nonsense"), GroupBy::Flat);
    assert_eq!(GroupBy::default(), GroupBy::Flat);
}
