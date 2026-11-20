use super::*;
use crate::test_support::{commit_file, git, init_repo};

#[test]
fn states_and_base_text() {
    let repo = init_repo();
    let path = repo.path();
    commit_file(path, "committed.txt", "base\n", "Initial");
    std::fs::write(path.join("added.txt"), "new\n").unwrap();
    git(path, &["add", "added.txt"]);
    std::fs::write(path.join("untracked.txt"), "x\n").unwrap();

    assert_eq!(file_state(path, "committed.txt"), Ok(FileState::Committed));
    assert_eq!(file_state(path, "added.txt"), Ok(FileState::Added));
    assert_eq!(file_state(path, "untracked.txt"), Ok(FileState::Untracked));

    std::fs::write(path.join("committed.txt"), "edited\n").unwrap();
    assert_eq!(
        base_text(path, "committed.txt"),
        Ok(Some("base\n".to_owned()))
    );
    assert_eq!(base_text(path, "added.txt"), Ok(Some(String::new())));
    assert_eq!(base_text(path, "untracked.txt"), Ok(None));
}

#[test]
fn rollback_restores_committed_files_and_unstages_new_ones() {
    let repo = init_repo();
    let path = repo.path();
    commit_file(path, "committed.txt", "base\n", "Initial");
    std::fs::write(path.join("committed.txt"), "edited\n").unwrap();
    git(path, &["add", "committed.txt"]);
    std::fs::write(path.join("committed.txt"), "edited again\n").unwrap();
    std::fs::write(path.join("added.txt"), "new\n").unwrap();
    git(path, &["add", "added.txt"]);

    assert_eq!(rollback(path, "committed.txt"), Ok(FileState::Committed));
    assert_eq!(
        std::fs::read_to_string(path.join("committed.txt")).unwrap(),
        "base\n"
    );
    assert_eq!(rollback(path, "added.txt"), Ok(FileState::Added));
    assert!(path.join("added.txt").exists(), "new files stay on disk");
    assert_eq!(git(path, &["status", "--porcelain"]), "?? added.txt\n");
}

#[test]
fn rollback_args_per_state() {
    assert_eq!(
        rollback_args(FileState::Committed, "a b.txt"),
        Some(vec![
            "checkout".to_owned(),
            "HEAD".to_owned(),
            "--".to_owned(),
            "a b.txt".to_owned()
        ])
    );
    assert_eq!(
        rollback_args(FileState::Added, "x").unwrap()[0..2],
        ["rm".to_owned(), "--cached".to_owned()]
    );
    assert_eq!(rollback_args(FileState::Untracked, "x"), None);
}
