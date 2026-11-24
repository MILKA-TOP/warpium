use super::*;

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "warpium-memo-test-{}-{}",
        name,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    dir
}

#[test]
fn caches_until_ref_stamp_changes() {
    let repo = temp_repo("stamp");
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

    assert_eq!(cached(&repo), None, "no entry before store");

    store(&repo, "main");
    assert_eq!(cached(&repo).as_deref(), Some("main"));

    // Setting HEAD's mtime to a fixed past time changes the stamp.
    let status = std::process::Command::new("touch")
        .arg("-t")
        .arg("202001020304.05")
        .arg(repo.join(".git/HEAD"))
        .status()
        .expect("touch runs");
    assert!(status.success());
    assert_eq!(
        cached(&repo),
        None,
        "a changed HEAD mtime invalidates the memo"
    );

    store(&repo, "trunk");
    assert_eq!(cached(&repo).as_deref(), Some("trunk"));
}

#[test]
fn new_ref_file_changes_the_stamp() {
    let repo = temp_repo("packed");
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    store(&repo, "main");
    assert_eq!(cached(&repo).as_deref(), Some("main"));
    // A fetch can rewrite packed-refs with a newer mtime; the stamp is the
    // newest mtime, so it must invalidate.
    std::fs::write(repo.join(".git/packed-refs"), "# pack-refs\n").unwrap();
    let status = std::process::Command::new("touch")
        .arg("-t")
        .arg("203001020304.05")
        .arg(repo.join(".git/packed-refs"))
        .status()
        .expect("touch runs");
    assert!(status.success());
    assert_eq!(cached(&repo), None);
}

#[test]
fn resolves_worktree_gitdir_pointer() {
    let repo = temp_repo("worktree");
    let gitdir = repo.parent().unwrap().join("warpium-memo-test-gitdir");
    let _ = std::fs::remove_dir_all(&gitdir);
    std::fs::create_dir_all(&gitdir).unwrap();
    // Replace the directory `temp_repo` created with a `.git` file pointer.
    std::fs::remove_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
    assert_eq!(resolve_git_dir(&repo), gitdir);

    let plain = temp_repo("plain");
    assert_eq!(resolve_git_dir(&plain), plain.join(".git"));
}
