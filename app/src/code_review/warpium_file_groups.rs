//! The changes-list row model behind the Code Review panel's Group By toolbar
//! (design doc §5.1) and its "Unversioned Files" group node (§5.4), modeled on
//! JetBrains' Commit tool window: a flat file list, a directory tree, or the
//! repository node as the structural root — with untracked files kept strictly
//! under a separate "Unversioned Files" node, never mixed into the changes.
//!
//! The rows are a flattening of the tree, the way the Git Log pane flattens
//! `warpium_git::tree::file_tree`, so the panel's viewported list keeps one
//! item per row in every mode.

/// The persisted Group By selection (`warpium.code_review_group_by`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupBy {
    /// One row per file, no grouping (the upstream shape).
    #[default]
    Flat,
    /// Collapsible directory nodes; chains of single-child directories are
    /// joined (`src/main/kotlin`), as in the Git Log pane.
    Directory,
    /// The repository node as the structural root, the flat list beneath it.
    /// Correct today and keyed by repo so a future multi-repo mode adds nodes
    /// without rework (design doc §5.1).
    Repository,
}

impl GroupBy {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Flat => "Flat",
            Self::Directory => "Directory",
            Self::Repository => "Repository",
        }
    }

    pub(crate) fn all() -> [Self; 3] {
        [Self::Flat, Self::Directory, Self::Repository]
    }

    /// Parses the persisted setting value; unknown values fall back to Flat.
    pub(crate) fn from_settings(value: &str) -> Self {
        match value {
            "directory" => Self::Directory,
            "repository" => Self::Repository,
            _ => Self::Flat,
        }
    }

    pub(crate) fn as_settings_value(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Directory => "directory",
            Self::Repository => "repository",
        }
    }
}

/// One row of the changes list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileRow {
    /// The structural repository node (Repository grouping's root). Its
    /// `path` is the repo's display name, also used as its toggle key.
    Repository {
        name: String,
        file_count: usize,
        expanded: bool,
    },
    /// A directory node in Directory grouping; `path` is the full joined
    /// directory path (`src/main/kotlin`), `label` what is shown.
    Directory {
        path: String,
        label: String,
        depth: usize,
        file_count: usize,
        expanded: bool,
    },
    /// The "Unversioned Files" group node untracked files live under.
    UnversionedNode {
        file_count: usize,
        expanded: bool,
    },
    /// A changed file row at `depth` under its group parents.
    File { path: String, depth: usize },
}

/// Builds the flattened rows for the changes list.
///
/// `files` are the repo-relative paths in display order with whether each is
/// untracked; tracked files come first, untracked last (the order the diff
/// model emits). `collapsed_dirs` holds the joined paths of collapsed
/// directory nodes; directories render expanded unless in the set, matching
/// JetBrains' tool window where the top level starts open. Untracked files
/// render under the "Unversioned Files" node in every mode.
pub(crate) fn build_rows(
    files: &[(String, bool)],
    group_by: GroupBy,
    collapsed_dirs: &std::collections::HashSet<String>,
    repository_node_expanded: bool,
    unversioned_expanded: bool,
) -> Vec<FileRow> {
    let tracked: Vec<&String> = files
        .iter()
        .filter(|(_, untracked)| !untracked)
        .map(|(path, _)| path)
        .collect();
    let untracked: Vec<&String> = files
        .iter()
        .filter(|(_, untracked)| *untracked)
        .map(|(path, _)| path)
        .collect();

    let mut rows = Vec::new();
    match group_by {
        GroupBy::Flat => {
            rows.extend(tracked.iter().map(|path| FileRow::File {
                path: (*path).clone(),
                depth: 0,
            }));
        }
        GroupBy::Directory => {
            push_directory_rows(&tracked, collapsed_dirs, 0, &mut rows);
        }
        GroupBy::Repository => {
            if !files.is_empty() {
                rows.push(FileRow::Repository {
                    name: String::new(),
                    file_count: tracked.len(),
                    expanded: repository_node_expanded,
                });
                if repository_node_expanded {
                    rows.extend(tracked.iter().map(|path| FileRow::File {
                        path: (*path).clone(),
                        depth: 1,
                    }));
                }
            }
        }
    }

    if !untracked.is_empty() {
        rows.push(FileRow::UnversionedNode {
            file_count: untracked.len(),
            expanded: unversioned_expanded,
        });
        if unversioned_expanded {
            let depth = if matches!(group_by, GroupBy::Repository) && repository_node_expanded {
                1
            } else {
                0
            };
            rows.extend(untracked.iter().map(|path| FileRow::File {
                path: (*path).clone(),
                depth,
            }));
        }
    }
    rows
}

/// Directory tree rows from `warpium_git::tree`, honoring each node's
/// expansion: a collapsed node's subtree is skipped, like the Git Log pane's
/// flattened tree.
fn push_directory_rows(
    tracked: &[&String],
    collapsed_dirs: &std::collections::HashSet<String>,
    base_depth: usize,
    rows: &mut Vec<FileRow>,
) {
    let changes: Vec<warpium_git::diff::FileChange> = tracked
        .iter()
        .map(|path| warpium_git::diff::FileChange {
            path: (*path).clone(),
            kind: warpium_git::diff::FileChangeKind::Modified,
        })
        .collect();
    // `file_tree` joins single-child chains into one row whose `label` is the
    // joined path relative to the row's parent (`main/kotlin` under `src`).
    // The collapse key is the full repo-relative path (`src/main/kotlin`), so
    // reconstruct it with a stack of ancestor labels.
    let mut dir_stack: Vec<(usize, String)> = Vec::new();
    let mut skip_below_depth: Option<usize> = None;
    for tree_row in warpium_git::tree::file_tree(&changes) {
        if let Some(hidden_depth) = skip_below_depth {
            if tree_row.depth > hidden_depth {
                continue;
            }
            skip_below_depth = None;
        }
        match tree_row.kind {
            warpium_git::tree::TreeRowKind::Directory { files } => {
                while dir_stack
                    .last()
                    .is_some_and(|(depth, _)| *depth >= tree_row.depth)
                {
                    dir_stack.pop();
                }
                let full_path = dir_stack
                    .iter()
                    .map(|(_, label)| label.as_str())
                    .chain(std::iter::once(tree_row.label.as_str()))
                    .collect::<Vec<_>>()
                    .join("/");
                let expanded = !collapsed_dirs.contains(&full_path);
                dir_stack.push((tree_row.depth, tree_row.label.clone()));
                rows.push(FileRow::Directory {
                    path: full_path,
                    label: tree_row.label,
                    depth: base_depth + tree_row.depth,
                    file_count: files,
                    expanded,
                });
                if !expanded {
                    skip_below_depth = Some(tree_row.depth);
                }
            }
            warpium_git::tree::TreeRowKind::File(index) => {
                rows.push(FileRow::File {
                    path: changes[index].path.clone(),
                    depth: base_depth + tree_row.depth,
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "warpium_file_groups_tests.rs"]
mod tests;
