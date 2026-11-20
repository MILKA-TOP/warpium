use super::*;
use crate::process::run_git;
use crate::test_support::{commit_file, git, init_repo};

fn head(repo: &std::path::Path) -> Oid {
    let output = run_git(repo, &["rev-parse".to_owned(), "HEAD".to_owned()]).unwrap();
    Oid::from_hex(String::from_utf8_lossy(&output).trim()).unwrap()
}

#[test]
fn args_per_action() {
    let oid = Oid::from_hex(&"a".repeat(40)).unwrap();
    let hex = oid.to_hex();
    assert_eq!(CommitAction::CherryPick.args(&oid), ["cherry-pick", &hex]);
    assert_eq!(
        CommitAction::Revert.args(&oid),
        ["revert", "--no-edit", &hex]
    );
    assert_eq!(
        CommitAction::Checkout.args(&oid),
        ["checkout", "--detach", &hex]
    );
    assert_eq!(
        CommitAction::Reset(ResetMode::Hard).args(&oid),
        ["reset", "--hard", &hex]
    );
    assert_eq!(
        CommitAction::Reset(ResetMode::Keep).args(&oid),
        ["reset", "--keep", &hex]
    );
    assert_eq!(
        CommitAction::NewBranch("feature/x".to_owned()).args(&oid),
        ["branch", "--", "feature/x", &hex]
    );
    assert_eq!(
        CommitAction::Reset(ResetMode::Mixed).done_message("abc"),
        "Reset the current branch to abc (mixed)"
    );
}

#[test]
fn ref_names() {
    assert_eq!(ref_name_error("feature/login"), None);
    assert_eq!(ref_name_error("v1.2.3"), None);
    for bad in [
        "", "-x", "a..b", "a b", "a~1", "x.lock", "dir/", "a@{b", "a:b",
    ] {
        assert!(ref_name_error(bad).is_some(), "{bad:?} should be rejected");
    }
}

#[test]
fn actions_run_against_a_real_repository() {
    let repo = init_repo();
    let path = repo.path();
    commit_file(path, "a.txt", "1\n", "One");
    let first = head(path);
    commit_file(path, "a.txt", "1\n2\n", "Two");
    let second = head(path);

    run_git(
        path,
        &CommitAction::NewBranch("old".to_owned()).args(&first),
    )
    .unwrap();
    run_git(path, &CommitAction::NewTag("v1".to_owned()).args(&first)).unwrap();
    assert!(git(path, &["branch", "--list", "old"]).contains("old"));
    assert!(git(path, &["tag", "--list"]).contains("v1"));

    run_git(path, &CommitAction::Revert.args(&second)).unwrap();
    assert_eq!(std::fs::read_to_string(path.join("a.txt")).unwrap(), "1\n");

    run_git(path, &CommitAction::Reset(ResetMode::Hard).args(&second)).unwrap();
    assert_eq!(head(path), second);

    run_git(path, &CommitAction::Checkout.args(&first)).unwrap();
    assert_eq!(head(path), first);
    assert_eq!(
        git(path, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "HEAD"
    );
}
