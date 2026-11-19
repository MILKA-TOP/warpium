//! What the Git Log's commit actions run, as JetBrains IDEs' Log context menu offers them.

use crate::oid::Oid;

/// How Reset Current Branch to Here treats the index and work tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    /// Keeps the index and work tree.
    Soft,
    /// Keeps the work tree, resets the index.
    Mixed,
    /// Discards local changes.
    Hard,
    /// Keeps local changes, refusing when they touch files that differ.
    Keep,
}

impl ResetMode {
    pub fn flag(self) -> &'static str {
        match self {
            Self::Soft => "--soft",
            Self::Mixed => "--mixed",
            Self::Hard => "--hard",
            Self::Keep => "--keep",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitAction {
    CherryPick,
    Revert,
    /// Checks the commit out as a detached `HEAD`.
    Checkout,
    Reset(ResetMode),
    NewBranch(String),
    NewTag(String),
}

impl CommitAction {
    /// The arguments after `git` that apply the action to `oid`.
    pub fn args(&self, oid: &Oid) -> Vec<String> {
        let hex = oid.to_hex();
        let args: Vec<&str> = match self {
            Self::CherryPick => vec!["cherry-pick", &hex],
            Self::Revert => vec!["revert", "--no-edit", &hex],
            Self::Checkout => vec!["checkout", "--detach", &hex],
            Self::Reset(mode) => vec!["reset", mode.flag(), &hex],
            Self::NewBranch(name) => vec!["branch", "--", name, &hex],
            Self::NewTag(name) => vec!["tag", "--", name, &hex],
        };
        args.into_iter().map(str::to_owned).collect()
    }

    /// Past-tense text for the toast after it succeeded.
    pub fn done_message(&self, short: &str) -> String {
        match self {
            Self::CherryPick => format!("Cherry-picked {short}"),
            Self::Revert => format!("Reverted {short}"),
            Self::Checkout => format!("Checked out {short}"),
            Self::Reset(mode) => format!(
                "Reset the current branch to {short} ({})",
                &mode.flag()[2..]
            ),
            Self::NewBranch(name) => format!("Created branch {name} at {short}"),
            Self::NewTag(name) => format!("Created tag {name} at {short}"),
        }
    }
}

/// Why `name` can't be a branch or tag name, or `None` when it can. A subset of
/// `git check-ref-format`'s rules, enough to catch typos before running git (which checks fully).
pub fn ref_name_error(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("Enter a name");
    }
    if name.starts_with('-') || name.starts_with('/') || name.ends_with('/') {
        return Some("A name can't start with '-' or '/', or end with '/'");
    }
    if name.ends_with('.') || name.ends_with(".lock") || name.contains("..") || name.contains("//")
    {
        return Some("A name can't contain '..' or '//', or end with '.' or '.lock'");
    }
    if name.contains("@{") || name == "@" {
        return Some("A name can't contain '@{'");
    }
    if name
        .chars()
        .any(|c| c.is_ascii_control() || " ~^:?*[\\".contains(c))
    {
        return Some("A name can't contain spaces or any of ~ ^ : ? * [ \\");
    }
    None
}

#[cfg(test)]
#[path = "actions_tests.rs"]
mod tests;
