//! Staging and patch export for a subset of files: JetBrains' "Add to VCS",
//! "Shelve Changes" and "Create Patch" actions from the Commit tool window,
//! plus the argument builders behind the commit dialog's selected-files
//! commit (`git add -A -- <paths>` + `git commit --only -- <paths>`).

use std::path::Path;

use crate::process::{GitError, run_git};

/// Largest untracked file inlined into a created patch; bigger files are
/// reported as skipped instead of ballooning the patch.
const MAX_UNTRACKED_FILE_BYTES: usize = 1_048_576;

/// Number of leading bytes examined when classifying an untracked file as
/// binary.
const BINARY_CHECK_BYTES: usize = 1_024;

/// Arguments staging exactly `paths` ("Add to VCS"): new, modified and
/// deleted files alike.
pub fn add_args(paths: &[String]) -> Vec<String> {
    let mut args: Vec<String> = ["add", "-A", "--"].map(str::to_owned).to_vec();
    args.extend(paths.iter().cloned());
    args
}

/// Stages exactly `paths` (`git add -A -- <paths>`).
pub fn add(repo: &Path, paths: &[String]) -> Result<(), GitError> {
    run_git(repo, &add_args(paths))?;
    Ok(())
}

/// Arguments shelving `paths`: a stash of just those paths, so the rest of
/// the working tree stays put (JetBrains' Shelve on a selection).
pub fn shelve_args(msg: &str, paths: &[String]) -> Vec<String> {
    let mut args: Vec<String> = ["stash", "push", "-u", "-m"]
        .map(str::to_owned)
        .to_vec();
    args.push(msg.to_owned());
    args.push("--".to_owned());
    args.extend(paths.iter().cloned());
    args
}

/// Shelves `paths` (`git stash push -u -m <msg> -- <paths>`).
pub fn shelve(repo: &Path, msg: &str, paths: &[String]) -> Result<(), GitError> {
    run_git(repo, &shelve_args(msg, paths))?;
    Ok(())
}

/// Arguments for the "stage everything" half of a commit: all changes, or
/// only `paths` when the commit dialog's file selection is a subset.
pub fn add_all_args(paths: Option<&[String]>) -> Vec<String> {
    let mut args: Vec<String> = ["add", "-A"].map(str::to_owned).to_vec();
    if let Some(paths) = paths {
        args.push("--".to_owned());
        args.extend(paths.iter().cloned());
    }
    args
}

/// Arguments for the staged-empty guard before a commit, scoped to `paths`
/// when the commit covers a subset.
pub fn staged_name_only_args(paths: Option<&[String]>) -> Vec<String> {
    let mut args: Vec<String> = ["--no-optional-locks", "diff", "--cached", "--name-only"]
        .map(str::to_owned)
        .to_vec();
    if let Some(paths) = paths {
        args.push("--".to_owned());
        args.extend(paths.iter().cloned());
    }
    args
}

/// Arguments committing the staged changes, or exactly `paths` when the
/// commit dialog's file selection is a subset: `--only` takes the named
/// paths' working-tree state and leaves everything else — including other
/// staged files — out of the commit.
pub fn commit_args(message: &str, amend: bool, paths: Option<&[String]>) -> Vec<String> {
    let mut args: Vec<String> = vec!["commit".to_owned()];
    if amend {
        args.push("--amend".to_owned());
    }
    if let Some(paths) = paths {
        args.push("--only".to_owned());
    }
    args.push("-m".to_owned());
    args.push(message.to_owned());
    if let Some(paths) = paths {
        args.push("--".to_owned());
        args.extend(paths.iter().cloned());
    }
    args
}

/// Arguments diffing the working tree against `HEAD` for `paths`, the base
/// of a created patch.
pub fn diff_head_args(paths: &[String]) -> Vec<String> {
    let mut args: Vec<String> = ["diff", "HEAD", "--"].map(str::to_owned).to_vec();
    args.extend(paths.iter().cloned());
    args
}

/// The synthesized all-additions patch of a new (untracked) file, in the
/// same shape `git diff` prints for added files.
pub fn new_file_patch(path: &str, content: &str) -> String {
    let line_count = content.lines().count();
    let mut patch = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{line_count} @@\n"
    );
    for line in content.lines() {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    patch
}

/// A created patch: the unified diff plus the repo-relative paths left out
/// (binary or oversized untracked files).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CreatedPatch {
    pub text: String,
    pub skipped: Vec<String>,
}

/// Creates the patch of the given files against `HEAD`: one `git diff` for
/// the tracked paths, with all-additions hunks synthesized from disk for the
/// untracked ones. Before the first commit (no `HEAD`) the tracked half falls
/// back to the staged plus unstaged diff, like the commit-message diff.
pub fn create_patch(
    repo: &Path,
    tracked: &[String],
    untracked: &[String],
) -> Result<CreatedPatch, GitError> {
    let mut patch = CreatedPatch::default();
    if !tracked.is_empty() {
        let has_head = run_git(repo, &["rev-parse".to_owned(), "--verify".to_owned(), "HEAD".to_owned()]).is_ok();
        if has_head {
            let output = run_git(repo, &diff_head_args(tracked))?;
            patch.text.push_str(&String::from_utf8_lossy(&output));
        } else {
            let mut staged: Vec<String> = ["diff", "--cached", "--"].map(str::to_owned).to_vec();
            staged.extend(tracked.iter().cloned());
            patch
                .text
                .push_str(&String::from_utf8_lossy(&run_git(repo, &staged)?));
            let mut unstaged: Vec<String> = ["diff", "--"].map(str::to_owned).to_vec();
            unstaged.extend(tracked.iter().cloned());
            patch
                .text
                .push_str(&String::from_utf8_lossy(&run_git(repo, &unstaged)?));
        }
    }
    for path in untracked {
        let file_path = repo.join(path);
        let Ok(bytes) = std::fs::read(&file_path) else {
            patch.skipped.push(path.clone());
            continue;
        };
        if bytes.len() > MAX_UNTRACKED_FILE_BYTES {
            patch.skipped.push(path.clone());
            continue;
        }
        let check_len = bytes.len().min(BINARY_CHECK_BYTES);
        if bytes[..check_len].contains(&0) {
            patch.skipped.push(path.clone());
            continue;
        }
        let Ok(content) = std::str::from_utf8(&bytes) else {
            patch.skipped.push(path.clone());
            continue;
        };
        patch.text.push_str(&new_file_patch(path, content));
    }
    Ok(patch)
}

#[cfg(test)]
#[path = "stage_tests.rs"]
mod tests;
