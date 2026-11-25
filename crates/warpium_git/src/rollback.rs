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

fn batch(args: &[&str], paths: Vec<&str>) -> Vec<String> {
    let mut batch: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    batch.extend(paths.into_iter().map(str::to_owned));
    batch
}

/// The git invocations rolling back `files` as few processes as possible: all
/// committed paths in one `git checkout HEAD -- …`, all staged-new paths in one
/// `git rm --cached --quiet -- …`, all untracked paths in one
/// `git clean -fd -- …` (JetBrains' rollback deletes unversioned files, and
/// `git clean` removes only the paths it is given).
pub fn rollback_batches(files: &[(&str, FileState)]) -> Vec<Vec<String>> {
    let paths_in = |state: FileState| -> Vec<&str> {
        files
            .iter()
            .filter_map(|(path, file_state)| (*file_state == state).then_some(*path))
            .collect()
    };
    let committed = paths_in(FileState::Committed);
    let added = paths_in(FileState::Added);
    let untracked = paths_in(FileState::Untracked);

    let mut batches = Vec::new();
    if !committed.is_empty() {
        batches.push(batch(&["checkout", "HEAD", "--"], committed));
    }
    if !added.is_empty() {
        batches.push(batch(&["rm", "--cached", "--quiet", "--"], added));
    }
    if !untracked.is_empty() {
        batches.push(batch(&["clean", "-fd", "--"], untracked));
    }
    batches
}

/// Rolls back `files` in at most three git invocations, one per file state
/// (see [`rollback_batches`]). Callers pass each file's [`FileState`]; when it
/// isn't known, compute it with [`file_state`] first.
pub fn rollback_many(repo: &Path, files: &[(&str, FileState)]) -> Result<(), GitError> {
    for args in rollback_batches(files) {
        run_git(repo, &args)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "rollback_tests.rs"]
mod tests;
