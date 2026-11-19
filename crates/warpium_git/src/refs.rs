//! Branch and tag labels from `git log --decorate=full` (`%D`) and `git for-each-ref`.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefKind {
    /// A detached `HEAD`.
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
    /// Anything else, such as `refs/stash`.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RefLabel {
    /// The short name (`main`, `origin/main`, `v1.0`).
    pub name: String,
    pub kind: RefKind,
    /// Whether `HEAD` points at this branch.
    pub is_head: bool,
}

/// Parses `%D` output: `HEAD -> refs/heads/main, refs/remotes/origin/main, tag: refs/tags/v1`.
/// Short names (without `--decorate=full`) count as local branches. Labels come back in display
/// order: `HEAD`'s branch first, then local branches, remote branches and tags.
pub fn parse_decorations(decorations: &str) -> Vec<RefLabel> {
    let mut labels = Vec::new();
    for part in decorations.split(", ") {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(target) = part.strip_prefix("HEAD -> ") {
            let (name, kind) = classify(target);
            labels.push(RefLabel {
                name,
                kind,
                is_head: true,
            });
        } else if part == "HEAD" {
            labels.push(RefLabel {
                name: "HEAD".to_owned(),
                kind: RefKind::Head,
                is_head: true,
            });
        } else if let Some(tag) = part.strip_prefix("tag: ") {
            labels.push(RefLabel {
                name: tag.strip_prefix("refs/tags/").unwrap_or(tag).to_owned(),
                kind: RefKind::Tag,
                is_head: false,
            });
        } else {
            let (name, kind) = classify(part);
            // `origin/HEAD` only repeats the remote's default branch.
            if kind == RefKind::RemoteBranch && name.ends_with("/HEAD") {
                continue;
            }
            labels.push(RefLabel {
                name,
                kind,
                is_head: false,
            });
        }
    }
    labels.sort_by_key(|label| (!label.is_head, label.kind));
    labels
}

fn classify(full_name: &str) -> (String, RefKind) {
    if let Some(name) = full_name.strip_prefix("refs/heads/") {
        (name.to_owned(), RefKind::LocalBranch)
    } else if let Some(name) = full_name.strip_prefix("refs/remotes/") {
        (name.to_owned(), RefKind::RemoteBranch)
    } else if let Some(name) = full_name.strip_prefix("refs/tags/") {
        (name.to_owned(), RefKind::Tag)
    } else if let Some(name) = full_name.strip_prefix("refs/") {
        (name.to_owned(), RefKind::Other)
    } else {
        (full_name.to_owned(), RefKind::LocalBranch)
    }
}

/// Arguments of `git for-each-ref` listing local and remote branches for [`parse_branch_list`].
pub fn branch_list_args() -> Vec<String> {
    [
        "for-each-ref",
        "--sort=-committerdate",
        "--format=%(HEAD)%(refname)",
        "refs/heads",
        "refs/remotes",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Branch names from [`branch_list_args`]' output: local branches, then remote ones, each group
/// most recently committed first. The checked-out branch has `is_head`.
pub fn parse_branch_list(output: &str) -> Vec<RefLabel> {
    let mut labels: Vec<RefLabel> = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (is_head, refname) = match line.strip_prefix('*') {
                Some(refname) => (true, refname),
                None => (false, line.trim_start()),
            };
            let (name, kind) = classify(refname.trim());
            RefLabel {
                name,
                kind,
                is_head,
            }
        })
        .filter(|label| !(label.kind == RefKind::RemoteBranch && label.name.ends_with("/HEAD")))
        .collect();
    labels.sort_by_key(|label| label.kind);
    labels
}

#[cfg(test)]
#[path = "refs_tests.rs"]
mod tests;
