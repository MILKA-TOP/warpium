use super::*;
use crate::diff::FileChangeKind;
use crate::test_support::{commit_file, git, init_repo};

fn stash_record(index: usize, message: &str) -> String {
    format!(
        "stash@{{{index}}}\u{1f}9911e3876c7128dde5476aa0141916d403365415\u{1f}2026-11-24 11:03:02 +0000\u{1f}{message}\0"
    )
}

#[test]
fn parses_list_records() {
    let output = format!(
        "{}{}",
        stash_record(1, "WIP on main: first"),
        stash_record(0, "On main: newest\n\nwith a body line")
    );
    let entries = parse_list(output.as_bytes());
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].index, 1);
    assert_eq!(entries[0].message, "WIP on main: first");
    assert_eq!(entries[0].created, "2026-11-24 11:03:02 +0000");
    assert_eq!(
        entries[0].oid.to_hex(),
        "9911e3876c7128dde5476aa0141916d403365415"
    );
    assert_eq!(entries[1].index, 0);
    assert_eq!(entries[1].message, "On main: newest\n\nwith a body line");
}

#[test]
fn parses_list_skips_garbage_records() {
    let output = "not-a-stash\0stash@{x}\u{1f}zz\0stash@{2}\0\0";
    assert!(parse_list(output.as_bytes()).is_empty());
}

#[test]
fn args_builders_name_the_stash() {
    assert_eq!(
        apply_args(3),
        ["stash", "apply", "stash@{3}"].map(str::to_owned)
    );
    assert_eq!(pop_args(0), ["stash", "pop", "stash@{0}"].map(str::to_owned));
    assert_eq!(
        drop_args(12),
        ["stash", "drop", "stash@{12}"].map(str::to_owned)
    );
    assert_eq!(
        show_patch_args(2),
        [
            "stash",
            "show",
            "--include-untracked",
            "--patch",
            "--no-color",
            "stash@{2}"
        ]
        .map(str::to_owned)
    );
    assert_eq!(
        push_args("my message"),
        ["stash", "push", "-u", "-m", "my message", "--"].map(str::to_owned)
    );
}

#[test]
fn parses_patch_sections_with_kinds() {
    let patch = "\
diff --git a/src/main.rs b/src/main.rs
index 0000000..1111111 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,2 +1,2 @@
-old
+new
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..2222222
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 3333333..0000000
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/old name.txt b/new name.txt
similarity index 100%
rename from old name.txt
rename to new name.txt
";
    let files = parse_patch(patch);
    assert_eq!(files.len(), 4);
    assert_eq!(files[0].path, "src/main.rs");
    assert_eq!(files[0].kind, FileChangeKind::Modified);
    assert!(files[0].patch.starts_with("diff --git a/src/main.rs"));
    assert_eq!(files[1].path, "new.txt");
    assert_eq!(files[1].kind, FileChangeKind::Added);
    assert_eq!(files[2].path, "gone.txt");
    assert_eq!(files[2].kind, FileChangeKind::Deleted);
    assert_eq!(files[3].path, "new name.txt");
    assert_eq!(
        files[3].kind,
        FileChangeKind::Renamed {
            from: "old name.txt".to_owned()
        }
    );
}

#[test]
fn list_create_apply_pop_drop_against_real_repo() {
    let repo = init_repo();
    let path = repo.path();
    commit_file(path, "tracked.txt", "base\n", "Initial");
    std::fs::write(path.join("tracked.txt"), "edited\n").unwrap();
    std::fs::write(path.join("untracked.txt"), "new\n").unwrap();

    create(path, "save my work").unwrap();
    // Stashing removed both edits from the working tree.
    assert_eq!(
        std::fs::read_to_string(path.join("tracked.txt")).unwrap(),
        "base\n"
    );
    assert!(!path.join("untracked.txt").exists());

    let entries = list_stashes(path).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].index, 0);
    assert!(entries[0].message.contains("save my work"));
    assert!(!entries[0].oid.is_zero());

    let files = stash_patch(path, 0).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|file| file.path == "tracked.txt"));
    assert!(files.iter().any(|file| file.path == "untracked.txt"));

    apply(path, 0).unwrap();
    assert_eq!(
        std::fs::read_to_string(path.join("tracked.txt")).unwrap(),
        "edited\n"
    );
    assert!(path.join("untracked.txt").exists());
    assert_eq!(list_stashes(path).unwrap().len(), 1, "apply keeps the stash");

    // Applying again would conflict with the restored working tree; reset it
    // before popping, which applies the stash and drops it.
    git(path, &["checkout", "--", "."]);
    std::fs::remove_file(path.join("untracked.txt")).unwrap();
    pop(path, 0).unwrap();
    assert_eq!(
        std::fs::read_to_string(path.join("tracked.txt")).unwrap(),
        "edited\n"
    );
    assert!(
        list_stashes(path).unwrap().is_empty(),
        "pop drops the stash"
    );

    create(path, "again").unwrap();
    drop(path, 0).unwrap();
    assert!(list_stashes(path).unwrap().is_empty());
    // Dropping restores nothing.
    assert_eq!(
        std::fs::read_to_string(path.join("tracked.txt")).unwrap(),
        "base\n"
    );
}

#[test]
fn stash_selector_tracks_index_after_drop() {
    let repo = init_repo();
    let path = repo.path();
    commit_file(path, "a.txt", "a\n", "Initial");
    for name in ["one", "two", "three"] {
        std::fs::write(path.join("a.txt"), format!("{name}\n")).unwrap();
        git(path, &["stash", "push", "-m", name]);
    }
    let entries = list_stashes(path).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].message, "On main: three");
    drop(path, 1).unwrap();
    let entries = list_stashes(path).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].message, "On main: one");
}
