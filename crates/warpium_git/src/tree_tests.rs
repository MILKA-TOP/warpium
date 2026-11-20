use super::*;
use crate::diff::FileChangeKind;

fn change(path: &str) -> FileChange {
    FileChange {
        path: path.to_owned(),
        kind: FileChangeKind::Modified,
    }
}

fn shape(rows: &[TreeRow]) -> Vec<(usize, &str, Option<usize>)> {
    rows.iter()
        .map(|row| {
            let file = match row.kind {
                TreeRowKind::File(index) => Some(index),
                TreeRowKind::Directory { .. } => None,
            };
            (row.depth, row.label.as_str(), file)
        })
        .collect()
}

#[test]
fn groups_by_directory_and_joins_single_child_chains() {
    let changes = [
        change("README.md"),
        change("app/src/main/kotlin/App.kt"),
        change("app/src/main/kotlin/ui/Screen.kt"),
        change("app/build.gradle.kts"),
        change("Cargo.toml"),
    ];
    let rows = file_tree(&changes);
    assert_eq!(
        shape(&rows),
        [
            (0, "app", None),
            (1, "src/main/kotlin", None),
            (2, "ui", None),
            (3, "Screen.kt", Some(2)),
            (2, "App.kt", Some(1)),
            (1, "build.gradle.kts", Some(3)),
            (0, "Cargo.toml", Some(4)),
            (0, "README.md", Some(0)),
        ]
    );
    assert_eq!(rows[0].kind, TreeRowKind::Directory { files: 3 });
}

#[test]
fn empty_and_flat() {
    assert!(file_tree(&[]).is_empty());
    let rows = file_tree(&[change("b"), change("a")]);
    assert_eq!(shape(&rows), [(0, "a", Some(1)), (0, "b", Some(0))]);
}
