//! The per-repo "which files go into the commit" selection behind the changes
//! list's checkboxes, matching JetBrains' Commit tool window: checked files
//! are committed, unchecked ones keep their changes uncommitted. Stored as
//! the *excluded* set so files appearing after a refresh start checked, as in
//! JetBrains; stale exclusions (files no longer changed) are harmless.

use std::collections::HashSet;

/// Tri-state of a group node's checkbox (a directory, the repository root or
/// the "Unversioned Files" node).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupCheckState {
    All,
    Some,
    None,
}

#[derive(Default)]
pub(crate) struct CommitSelection {
    /// Repo-relative paths the user unchecked.
    excluded: HashSet<String>,
}

impl CommitSelection {
    pub(crate) fn is_included(&self, path: &str) -> bool {
        !self.excluded.contains(path)
    }

    pub(crate) fn toggle(&mut self, path: &str) {
        if !self.excluded.insert(path.to_owned()) {
            self.excluded.remove(path);
        }
    }

    pub(crate) fn set_included(&mut self, paths: &[String], included: bool) {
        for path in paths {
            if included {
                self.excluded.remove(path);
            } else {
                self.excluded.insert(path.clone());
            }
        }
    }

    /// The paths a commit should cover: `None` when every changed file is
    /// included — the legacy whole-tree commit — else `Some` of the included
    /// subset, in the changes list's order. Exclusions of files that are no
    /// longer changed don't turn the commit into a subset.
    pub(crate) fn commit_subset(&self, changed: &[String]) -> Option<Vec<String>> {
        if self.excluded.is_empty() {
            return None;
        }
        let included: Vec<String> = changed
            .iter()
            .filter(|path| self.is_included(path))
            .cloned()
            .collect();
        if included.len() == changed.len() {
            None
        } else {
            Some(included)
        }
    }

    pub(crate) fn group_state(&self, paths: &[String]) -> GroupCheckState {
        let included = paths.iter().filter(|path| self.is_included(path)).count();
        if included == 0 {
            GroupCheckState::None
        } else if included == paths.len() {
            GroupCheckState::All
        } else {
            GroupCheckState::Some
        }
    }
}

#[cfg(test)]
#[path = "warpium_commit_selection_tests.rs"]
mod tests;
