//! A process-wide memo of the repository's main branch name.
//!
//! [`detect_main_branch`](crate::util::git::detect_main_branch) can cost up to
//! seven sequential `git` spawns, and three separate loads run it every time
//! the Code Review panel opens (design: `docs/warpium/research/code-review-panel.md`
//! §4.9 F3). The answer only changes when the repo's refs change, so the memo
//! keys each repo to a stamp of the git files whose modification can affect it:
//! `HEAD` (branch switches), `packed-refs`, and `refs/remotes/origin/HEAD`
//! (the origin default branch), resolved through a worktree's `gitdir:` pointer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

#[derive(Default)]
struct MainBranchMemo {
    entries: HashMap<PathBuf, Entry>,
}

#[derive(Clone)]
struct Entry {
    stamp: SystemTime,
    name: String,
}

static MEMO: Mutex<Option<MainBranchMemo>> = Mutex::new(None);

/// Returns the memoized main branch name for `repo_path` while the repo's ref
/// stamp is unchanged, or `None` on a miss or a stale stamp.
pub(crate) fn cached(repo_path: &Path) -> Option<String> {
    let mut memo = MEMO.lock().unwrap_or_else(PoisonError::into_inner);
    let memo = memo.get_or_insert_with(MainBranchMemo::default);
    let entry = memo.entries.get(repo_path)?;
    (entry.stamp == stamp(repo_path)).then(|| entry.name.clone())
}

/// Stores `name` as the main branch of `repo_path` under the current stamp.
pub(crate) fn store(repo_path: &Path, name: &str) {
    let mut memo = MEMO.lock().unwrap_or_else(PoisonError::into_inner);
    let memo = memo.get_or_insert_with(MainBranchMemo::default);
    memo.entries.insert(
        repo_path.to_path_buf(),
        Entry {
            stamp: stamp(repo_path),
            name: name.to_owned(),
        },
    );
}

/// The newest mtime of the git files whose change can alter the detected main
/// branch; the UNIX epoch when none exist (yet).
fn stamp(repo_path: &Path) -> SystemTime {
    let git_dir = resolve_git_dir(repo_path);
    let candidates = [
        git_dir.join("HEAD"),
        git_dir.join("packed-refs"),
        git_dir.join("refs/remotes/origin/HEAD"),
    ];
    candidates
        .iter()
        .filter_map(|path| path.metadata().and_then(|meta| meta.modified()).ok())
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// The git directory of `repo_path`, following a worktree/submodule's
/// `.git` file pointer.
fn resolve_git_dir(repo_path: &Path) -> PathBuf {
    let dot_git = repo_path.join(".git");
    if dot_git.is_file()
        && let Some(target) = std::fs::read_to_string(&dot_git)
            .ok()
            .and_then(|contents| {
                contents
                    .trim()
                    .strip_prefix("gitdir:")
                    .map(str::trim)
                    .map(PathBuf::from)
            })
    {
        return target;
    }
    dot_git
}

#[cfg(test)]
#[path = "warpium_main_branch_tests.rs"]
mod tests;
