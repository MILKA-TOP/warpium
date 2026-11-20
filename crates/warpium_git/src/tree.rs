//! A commit's changed files as a directory tree, as JetBrains IDEs show them: directories first,
//! chains of single-child directories joined (`src/main/kotlin`), then files, each sorted by name.

use std::collections::BTreeMap;

use crate::diff::FileChange;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeRowKind {
    /// A directory, with the number of files below it.
    Directory { files: usize },
    /// A file: an index into the changes the tree was built from.
    File(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: usize,
    /// The directory path (joined chain) or the file name.
    pub label: String,
    pub kind: TreeRowKind,
}

#[derive(Default)]
struct Node {
    directories: BTreeMap<String, Node>,
    files: Vec<(String, usize)>,
}

impl Node {
    fn file_count(&self) -> usize {
        self.files.len()
            + self
                .directories
                .values()
                .map(Node::file_count)
                .sum::<usize>()
    }
}

/// The rows of the tree of `changes`, in display order.
pub fn file_tree(changes: &[FileChange]) -> Vec<TreeRow> {
    let mut root = Node::default();
    for (index, change) in changes.iter().enumerate() {
        let mut parts: Vec<&str> = change.path.split('/').collect();
        let name = parts.pop().unwrap_or_default().to_owned();
        let mut node = &mut root;
        for part in parts {
            node = node.directories.entry(part.to_owned()).or_default();
        }
        node.files.push((name, index));
    }
    let mut rows = Vec::new();
    push_rows(&root, 0, &mut rows);
    rows
}

fn push_rows(node: &Node, depth: usize, rows: &mut Vec<TreeRow>) {
    for (name, child) in &node.directories {
        let mut label = name.clone();
        let mut child = child;
        while child.files.is_empty() && child.directories.len() == 1 {
            let Some((next_name, next)) = child.directories.iter().next() else {
                break;
            };
            label.push('/');
            label.push_str(next_name);
            child = next;
        }
        rows.push(TreeRow {
            depth,
            label,
            kind: TreeRowKind::Directory {
                files: child.file_count(),
            },
        });
        push_rows(child, depth + 1, rows);
    }
    let mut files = node.files.clone();
    files.sort();
    for (name, index) in files {
        rows.push(TreeRow {
            depth,
            label: name,
            kind: TreeRowKind::File(index),
        });
    }
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
