//! A file's committed version (the base of the editor's change markers) and rolling a file back
//! to it, as JetBrains IDEs' Rollback does.

use std::path::Path;

use crate::process::{GitError, run_git};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileState {
    /// The file is in `HEAD`.
    Committed,
    /// The file is staged but not in `HEAD` (a new file, `git add`ed).
    Added,
    /// git doesn't track the file (or ignores it).
    Untracked,
}

/// Whether `relative_path` is in `HEAD`, only staged, or untracked.
pub fn file_state(repo: &Path, relative_path: &str) -> Result<FileState, GitError> {
    if run_git(repo, &head_object_args(relative_path, "-e")).is_ok() {
        return Ok(FileState::Committed);
    }
    let tracked = run_git(
        repo,
        &[
            "ls-files".to_owned(),
            "--error-unmatch".to_owned(),
            "--".to_owned(),
            relative_path.to_owned(),
        ],
    );
    match tracked {
        Ok(_) => Ok(FileState::Added),
        Err(GitError::Failed { .. }) => Ok(FileState::Untracked),
        Err(err @ GitError::Spawn(_)) => Err(err),
    }
}

fn head_object_args(relative_path: &str, mode: &str) -> Vec<String> {
    vec![
        "cat-file".to_owned(),
        mode.to_owned(),
        format!("HEAD:{relative_path}"),
    ]
}

/// The text the editor's change markers compare against: the file in `HEAD`, empty for a file
/// that is only staged (every line is new), or `None` for an untracked file, which gets no
/// markers.
pub fn base_text(repo: &Path, relative_path: &str) -> Result<Option<String>, GitError> {
    match run_git(repo, &head_object_args(relative_path, "blob")) {
        Ok(blob) => Ok(Some(String::from_utf8_lossy(&blob).into_owned())),
        Err(GitError::Failed { .. }) => match file_state(repo, relative_path)? {
            FileState::Added => Ok(Some(String::new())),
            FileState::Committed | FileState::Untracked => Ok(None),
        },
        Err(err @ GitError::Spawn(_)) => Err(err),
    }
}

/// What rolling back a file in `state` runs: restore the committed version in the index and the
/// work tree, or unstage a new file (keeping it on disk). Untracked files have nothing to roll
/// back to.
pub fn rollback_args(state: FileState, relative_path: &str) -> Option<Vec<String>> {
    let args: &[&str] = match state {
        FileState::Committed => &["checkout", "HEAD", "--"],
        FileState::Added => &["rm", "--cached", "--quiet", "--"],
        FileState::Untracked => return None,
    };
    let mut args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    args.push(relative_path.to_owned());
    Some(args)
}

/// Rolls `relative_path` back to `HEAD` and returns the state it was in.
pub fn rollback(repo: &Path, relative_path: &str) -> Result<FileState, GitError> {
    let state = file_state(repo, relative_path)?;
    if let Some(args) = rollback_args(state, relative_path) {
        run_git(repo, &args)?;
    }
    Ok(state)
}

#[cfg(test)]
#[path = "rollback_tests.rs"]
mod tests;
