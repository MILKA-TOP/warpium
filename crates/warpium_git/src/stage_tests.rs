use super::*;
use crate::test_support::{commit_file, git, init_repo};

#[test]
fn add_args_stage_exactly_the_given_paths() {
    assert_eq!(
        add_args(&["a.txt".to_owned(), "dir/b.txt".to_owned()]),
        ["add", "-A", "--", "a.txt", "dir/b.txt"].map(str::to_owned)
    );
}

#[test]
fn shelve_args_stash_push_the_paths() {
    assert_eq!(
        shelve_args("msg", &["a.txt".to_owned()]),
        ["stash", "push", "-u", "-m", "msg", "--", "a.txt"].map(str::to_owned)
    );
}

#[test]
fn add_all_args_cover_all_or_a_subset() {
    assert_eq!(add_all_args(None), ["add", "-A"].map(str::to_owned));
    assert_eq!(
        add_all_args(Some(&["a.txt".to_owned()])),
        ["add", "-A", "--", "a.txt"].map(str::to_owned)
    );
}

#[test]
fn staged_name_only_args_scope_to_the_subset() {
    assert_eq!(
        staged_name_only_args(None),
        ["--no-optional-locks", "diff", "--cached", "--name-only"].map(str::to_owned)
    );
    assert_eq!(
        staged_name_only_args(Some(&["a.txt".to_owned()])),
        ["--no-optional-locks", "diff", "--cached", "--name-only", "--", "a.txt"]
            .map(str::to_owned)
    );
}

#[test]
fn commit_args_use_only_for_a_subset() {
    assert_eq!(
        commit_args("msg", false, None),
        ["commit", "-m", "msg"].map(str::to_owned)
    );
    assert_eq!(
        commit_args("msg", true, None),
        ["commit", "--amend", "-m", "msg"].map(str::to_owned)
    );
    assert_eq!(
        commit_args("msg", false, Some(&["a.txt".to_owned(), "b.txt".to_owned()])),
        ["commit", "--only", "-m", "msg", "--", "a.txt", "b.txt"].map(str::to_owned)
    );
    assert_eq!(
        commit_args("msg", true, Some(&["a.txt".to_owned()])),
        ["commit", "--amend", "--only", "-m", "msg", "--", "a.txt"].map(str::to_owned)
    );
}

#[test]
fn new_file_patch_is_an_all_additions_hunk() {
    let patch = new_file_patch("src/new.rs", "fn main() {}\n// done\n");
    assert_eq!(
        patch,
        "diff --git a/src/new.rs b/src/new.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1,2 @@\n+fn main() {}\n+// done\n"
    );
}

#[test]
fn add_stages_only_the_named_paths() {
    let repo = init_repo();
    commit_file(repo.path(), "a.txt", "a\n", "init");
    std::fs::write(repo.path().join("a.txt"), "a\na2\n").expect("write");
    std::fs::write(repo.path().join("b.txt"), "b\n").expect("write");
    std::fs::write(repo.path().join("c.txt"), "c\n").expect("write");

    add(repo.path(), &["a.txt".to_owned(), "b.txt".to_owned()]).expect("add");

    let staged = git(repo.path(), &["diff", "--cached", "--name-only"]);
    assert_eq!(staged.lines().collect::<Vec<_>>(), ["a.txt", "b.txt"]);
    let untracked = git(repo.path(), &["ls-files", "--others", "--exclude-standard"]);
    assert_eq!(untracked.trim(), "c.txt");
}

#[test]
fn commit_only_commits_the_named_paths_and_keeps_the_rest() {
    let repo = init_repo();
    commit_file(repo.path(), "a.txt", "a\n", "init");
    commit_file(repo.path(), "b.txt", "b\n", "add b");
    std::fs::write(repo.path().join("a.txt"), "a\na2\n").expect("write");
    std::fs::write(repo.path().join("b.txt"), "b\nb2\n").expect("write");
    std::fs::write(repo.path().join("d.txt"), "d\n").expect("write");
    // An unrelated staged file must stay out of a subset commit.
    git(repo.path(), &["add", "b.txt"]);

    add(repo.path(), &["a.txt".to_owned(), "d.txt".to_owned()]).expect("add");
    let args = commit_args("subset", false, Some(&["a.txt".to_owned(), "d.txt".to_owned()]));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    git(repo.path(), &arg_refs);

    let committed = git(repo.path(), &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(
        committed.lines().collect::<Vec<_>>(),
        ["a.txt", "d.txt"]
    );
    // b.txt's staged change survives, uncommitted.
    let staged = git(repo.path(), &["diff", "--cached", "--name-only"]);
    assert_eq!(staged.trim(), "b.txt");
}

#[test]
fn commit_only_stages_a_deletion() {
    let repo = init_repo();
    commit_file(repo.path(), "a.txt", "a\n", "init");
    commit_file(repo.path(), "b.txt", "b\n", "add b");
    std::fs::remove_file(repo.path().join("b.txt")).expect("remove");

    add(repo.path(), &["b.txt".to_owned()]).expect("add");
    let args = commit_args("delete b", false, Some(&["b.txt".to_owned()]));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    git(repo.path(), &arg_refs);

    let names = git(repo.path(), &["ls-files"]);
    assert_eq!(names.trim(), "a.txt");
}

#[test]
fn shelve_stashes_only_the_named_paths() {
    let repo = init_repo();
    commit_file(repo.path(), "a.txt", "a\n", "init");
    commit_file(repo.path(), "b.txt", "b\n", "add b");
    std::fs::write(repo.path().join("a.txt"), "a\na2\n").expect("write");
    std::fs::write(repo.path().join("b.txt"), "b\nb2\n").expect("write");
    std::fs::write(repo.path().join("new.txt"), "new\n").expect("write");

    shelve(repo.path(), "shelve a", &["a.txt".to_owned(), "new.txt".to_owned()])
        .expect("shelve");

    // a.txt restored, b.txt's edit and the untracked file stay... a.txt is
    // stashed away (back to HEAD), b.txt keeps its edit; the untracked file
    // went into the stash with -u.
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).expect("read a"),
        "a\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("b.txt")).expect("read b"),
        "b\nb2\n"
    );
    assert!(!repo.path().join("new.txt").exists());
    let list = git(repo.path(), &["stash", "list"]);
    assert!(list.contains("shelve a"), "stash list: {list}");
}

#[test]
fn create_patch_diffs_tracked_and_synthesizes_untracked() {
    let repo = init_repo();
    commit_file(repo.path(), "a.txt", "a\n", "init");
    std::fs::write(repo.path().join("a.txt"), "a\na2\n").expect("write");
    std::fs::write(repo.path().join("new.txt"), "hello\n").expect("write");

    let patch = create_patch(
        repo.path(),
        &["a.txt".to_owned()],
        &["new.txt".to_owned()],
    )
    .expect("patch");

    assert!(patch.skipped.is_empty());
    assert!(patch.text.contains("diff --git a/a.txt b/a.txt"));
    assert!(patch.text.contains("+a2"));
    assert!(patch.text.contains("diff --git a/new.txt b/new.txt"));
    assert!(patch.text.contains("new file mode 100644"));
    assert!(patch.text.contains("+hello"));
}

#[test]
fn create_patch_skips_binary_and_unreadable_untracked_files() {
    let repo = init_repo();
    commit_file(repo.path(), "a.txt", "a\n", "init");
    std::fs::write(repo.path().join("bin.dat"), [0u8, 159, 146, 150]).expect("write");

    let patch = create_patch(
        repo.path(),
        &[],
        &["bin.dat".to_owned(), "missing.txt".to_owned()],
    )
    .expect("patch");

    assert!(patch.text.is_empty());
    assert_eq!(patch.skipped, ["bin.dat".to_owned(), "missing.txt".to_owned()]);
}

#[test]
fn create_patch_before_the_first_commit_uses_the_staged_diff() {
    let repo = init_repo();
    std::fs::write(repo.path().join("a.txt"), "a\n").expect("write");
    git(repo.path(), &["add", "a.txt"]);
    std::fs::write(repo.path().join("a.txt"), "a\na2\n").expect("write");

    let patch = create_patch(repo.path(), &["a.txt".to_owned()], &[]).expect("patch");

    assert!(patch.text.contains("diff --git a/a.txt b/a.txt"));
    assert!(patch.text.contains("+a"));
    assert!(patch.text.contains("+a2"));
}
