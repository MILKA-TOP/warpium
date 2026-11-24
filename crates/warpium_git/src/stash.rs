//! Stashes: listing `git stash` entries the way JetBrains IDEs' Git tool window
//! shows them (message, date, commit), and the create/apply/pop/drop operations
//! plus a per-file patch view of a stash's combined diff.

use std::path::Path;

use crate::oid::Oid;
use crate::process::{GitError, run_git};

/// One `stash@{n}` entry: its message, creation date and commit id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashEntry {
    /// The `n` in `stash@{n}`; applying/dropping addresses the entry by this.
    pub index: usize,
    pub message: String,
    /// The committer date as git formats `%ci` (`2026-11-24 11:03:02 +0000`).
    pub created: String,
    pub oid: Oid,
}

/// Arguments listing stashes as NUL-terminated records with unit-separated
/// fields: `stash@{n} <unit> <oid> <unit> <date> <unit> <message>`.
pub fn list_args() -> Vec<String> {
    ["stash", "list", "--format=%gd%x1f%H%x1f%ci%x1f%B", "-z"]
        .map(str::to_owned)
        .to_vec()
}

/// Parses the output of [`list_args`].
pub fn parse_list(output: &[u8]) -> Vec<StashEntry> {
    output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let record = String::from_utf8_lossy(record);
            let mut fields = record.split('\u{1f}');
            let selector = fields.next()?;
            let index = selector
                .trim_start_matches("stash@{")
                .trim_end_matches('}')
                .parse()
                .ok()?;
            let oid = Oid::from_hex(fields.next()?)?;
            let created = fields.next()?.to_owned();
            let message = fields.next().unwrap_or_default().trim().to_owned();
            Some(StashEntry {
                index,
                message,
                created,
                oid,
            })
        })
        .collect()
}

/// Lists the repo's stashes, newest first (`stash@{0}` is the most recent).
pub fn list_stashes(repo: &Path) -> Result<Vec<StashEntry>, GitError> {
    let output = run_git(repo, &list_args())?;
    Ok(parse_list(&output))
}

/// Arguments stashing the working tree (including untracked files) with `msg`.
pub fn push_args(msg: &str) -> Vec<String> {
    let mut args: Vec<String> = ["stash", "push", "-u", "-m"].map(str::to_owned).to_vec();
    args.push(msg.to_owned());
    args.push("--".to_owned());
    args
}

/// Stashes all local changes including untracked files.
pub fn create(repo: &Path, msg: &str) -> Result<(), GitError> {
    run_git(repo, &push_args(msg))?;
    Ok(())
}

/// Arguments applying `stash@{index}` without dropping it.
pub fn apply_args(index: usize) -> Vec<String> {
    ["stash", "apply", &format!("stash@{{{index}}}")]
        .map(str::to_owned)
        .to_vec()
}

/// Applies `stash@{index}` to the working tree, keeping the stash.
pub fn apply(repo: &Path, index: usize) -> Result<(), GitError> {
    run_git(repo, &apply_args(index))?;
    Ok(())
}

/// Arguments applying `stash@{index}` and dropping it on success.
pub fn pop_args(index: usize) -> Vec<String> {
    ["stash", "pop", &format!("stash@{{{index}}}")]
        .map(str::to_owned)
        .to_vec()
}

/// Applies `stash@{index}` and drops it.
pub fn pop(repo: &Path, index: usize) -> Result<(), GitError> {
    run_git(repo, &pop_args(index))?;
    Ok(())
}

/// Arguments dropping `stash@{index}`.
pub fn drop_args(index: usize) -> Vec<String> {
    ["stash", "drop", &format!("stash@{{{index}}}")]
        .map(str::to_owned)
        .to_vec()
}

/// Drops `stash@{index}`.
pub fn drop(repo: &Path, index: usize) -> Result<(), GitError> {
    run_git(repo, &drop_args(index))?;
    Ok(())
}

/// Arguments showing `stash@{index}`'s combined patch, untracked files included.
pub fn show_patch_args(index: usize) -> Vec<String> {
    [
        "stash",
        "show",
        "--include-untracked",
        "--patch",
        "--no-color",
        &format!("stash@{{{index}}}"),
    ]
    .map(str::to_owned)
    .to_vec()
}

/// The patch of one file inside a stash's combined `git stash show --patch` output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashFilePatch {
    /// The file's path in the working tree, relative to the repository root.
    pub path: String,
    pub kind: crate::diff::FileChangeKind,
    /// The file's section of the combined patch, from `diff --git` on.
    pub patch: String,
}

/// Splits a combined `git stash show --patch` output into per-file patches.
///
/// Each section starts at a `diff --git a/<old> b/<new>` line. The file path is
/// taken from the section's own headers: `+++ b/<path>` for additions and
/// modifications, `--- a/<path>` for deletions (where `+++` is `/dev/null`),
/// and `rename to` for renames. The output of `parse_unified_diff` renders each
/// `patch`.
pub fn parse_patch(output: &str) -> Vec<StashFilePatch> {
    #[derive(Default)]
    struct Section {
        old_path: Option<String>,
        new_path: Option<String>,
        kind: Option<crate::diff::FileChangeKind>,
        patch: String,
    }

    let mut files = Vec::new();
    let mut current = Section::default();

    let mut flush = |current: &mut Section| {
        let path = match &current.kind {
            Some(crate::diff::FileChangeKind::Renamed { from }) => {
                // The working-tree path is the rename target.
                (current.new_path.clone()).unwrap_or_else(|| from.clone())
            }
            Some(crate::diff::FileChangeKind::Deleted) => {
                current.old_path.clone().unwrap_or_default()
            }
            _ => current.new_path.clone().unwrap_or_default(),
        };
        if current.kind.is_some() || !current.patch.is_empty() {
            files.push(StashFilePatch {
                path,
                kind: current
                    .kind
                    .clone()
                    .unwrap_or(crate::diff::FileChangeKind::Modified),
                patch: std::mem::take(&mut current.patch),
            });
            *current = Section::default();
        }
    };

    for line in output.lines() {
        if line.starts_with("diff --git ") {
            flush(&mut current);
        }
        if let Some(old) = line.strip_prefix("--- a/") {
            current.old_path = Some(old.to_owned());
        } else if let Some(new) = line.strip_prefix("+++ b/") {
            current.new_path = Some(new.to_owned());
        } else if line.starts_with("new file mode ") {
            current.kind = Some(crate::diff::FileChangeKind::Added);
        } else if line.starts_with("deleted file mode ") {
            current.kind = Some(crate::diff::FileChangeKind::Deleted);
        } else if let Some(from) = line.strip_prefix("rename from ") {
            let from = from.to_owned();
            current.kind = Some(crate::diff::FileChangeKind::Renamed { from });
        } else if let Some(to) = line.strip_prefix("rename to ") {
            current.new_path = Some(to.to_owned());
        }
        current.patch.push_str(line);
        current.patch.push('\n');
    }
    flush(&mut current);
    files
}

/// The combined patch of `stash@{index}`, split per file (untracked included).
pub fn stash_patch(repo: &Path, index: usize) -> Result<Vec<StashFilePatch>, GitError> {
    let output = run_git(repo, &show_patch_args(index))?;
    Ok(parse_patch(&String::from_utf8_lossy(&output)))
}

#[cfg(test)]
#[path = "stash_tests.rs"]
mod tests;
