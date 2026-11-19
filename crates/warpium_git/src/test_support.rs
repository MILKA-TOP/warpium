//! Temporary repositories for tests that run the real git.

use std::path::Path;
use std::process::Stdio;

use command::blocking::Command;
use tempfile::TempDir;

pub(crate) fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "Ada Lovelace")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada Lovelace")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", repo)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(crate) fn init_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    git(dir.path(), &["init", "--quiet"]);
    dir
}

pub(crate) fn commit_file(repo: &Path, file: &str, contents: &str, message: &str) {
    std::fs::write(repo.join(file), contents).expect("write file");
    git(repo, &["add", file]);
    git(repo, &["commit", "--quiet", "-m", message]);
}

/// Appends `count` empty commits to `main` quickly with `git fast-import`.
pub(crate) fn fast_import_commits(repo: &Path, count: usize) {
    let mut stream = String::new();
    for index in 1..=count {
        stream.push_str("commit refs/heads/main\n");
        stream.push_str(&format!("mark :{index}\n"));
        stream.push_str(&format!(
            "committer Ada <ada@example.com> {} +0000\n",
            1_600_000_000 + index
        ));
        let message = format!("commit {index}");
        stream.push_str(&format!("data {}\n{message}\n", message.len()));
        if index > 1 {
            stream.push_str(&format!("from :{}\n", index - 1));
        }
        stream.push('\n');
    }
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(repo)
        .stdin(Stdio::piped())
        .spawn()
        .expect("fast-import runs");
    std::io::Write::write_all(child.stdin.as_mut().expect("stdin"), stream.as_bytes())
        .expect("write stream");
    assert!(child.wait().expect("fast-import exits").success());
    git(repo, &["reset", "--quiet", "--hard", "main"]);
}
