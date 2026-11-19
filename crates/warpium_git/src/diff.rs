//! A commit's changed files (`git diff-tree --name-status -z`) and unified diffs.

use crate::oid::Oid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
    Copied { from: String },
    TypeChanged,
    Other(char),
}

impl FileChangeKind {
    /// The status letter git uses.
    pub fn letter(&self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
            Self::Renamed { .. } => 'R',
            Self::Copied { .. } => 'C',
            Self::TypeChanged => 'T',
            Self::Other(letter) => *letter,
        }
    }

    /// The path before the change, for renames and copies.
    pub fn old_path(&self) -> Option<&str> {
        match self {
            Self::Renamed { from } | Self::Copied { from } => Some(from),
            Self::Added | Self::Modified | Self::Deleted | Self::TypeChanged | Self::Other(_) => {
                None
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// The path after the change, relative to the repository root.
    pub path: String,
    pub kind: FileChangeKind,
}

/// Arguments listing the files `commit` changed, against its first parent (merges included).
pub fn changed_files_args(commit: &Oid, first_parent: Option<&Oid>) -> Vec<String> {
    let mut args: Vec<String> = [
        "diff-tree",
        "-r",
        "-z",
        "-M",
        "--name-status",
        "--no-commit-id",
        "--no-color",
    ]
    .map(str::to_owned)
    .to_vec();
    match first_parent {
        Some(parent) => args.push(parent.to_hex()),
        None => args.push("--root".to_owned()),
    }
    args.push(commit.to_hex());
    args
}

/// Parses `--name-status -z` output: `M\0path\0`, `R100\0old\0new\0`.
pub fn parse_name_status(output: &[u8]) -> Vec<FileChange> {
    let mut fields = output
        .split(|byte| *byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned());
    let mut changes = Vec::new();
    while let Some(status) = fields.next() {
        let Some(letter) = status.chars().next() else {
            continue;
        };
        let Some(path) = fields.next() else {
            break;
        };
        let change = match letter {
            'R' | 'C' => {
                let Some(new_path) = fields.next().filter(|path| !path.is_empty()) else {
                    break;
                };
                let kind = if letter == 'R' {
                    FileChangeKind::Renamed { from: path }
                } else {
                    FileChangeKind::Copied { from: path }
                };
                FileChange {
                    path: new_path,
                    kind,
                }
            }
            'A' => FileChange {
                path,
                kind: FileChangeKind::Added,
            },
            'M' => FileChange {
                path,
                kind: FileChangeKind::Modified,
            },
            'D' => FileChange {
                path,
                kind: FileChangeKind::Deleted,
            },
            'T' => FileChange {
                path,
                kind: FileChangeKind::TypeChanged,
            },
            other => FileChange {
                path,
                kind: FileChangeKind::Other(other),
            },
        };
        changes.push(change);
    }
    changes
}

/// Arguments of the patch of one file in `commit` against its first parent.
pub fn file_diff_args(
    commit: &Oid,
    first_parent: Option<&Oid>,
    change: &FileChange,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "diff-tree",
        "-p",
        "-M",
        "--no-commit-id",
        "--no-color",
        "--no-ext-diff",
    ]
    .map(str::to_owned)
    .to_vec();
    match first_parent {
        Some(parent) => args.push(parent.to_hex()),
        None => args.push("--root".to_owned()),
    }
    args.push(commit.to_hex());
    args.push("--".to_owned());
    if let Some(old_path) = change.kind.old_path() {
        args.push(old_path.to_owned());
    }
    args.push(change.path.clone());
    args
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    /// `@@ -1,3 +1,4 @@`.
    Hunk,
    Context,
    Added,
    Removed,
    /// Anything else worth showing, such as "Binary files differ" or a rename header.
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// 1-based line numbers before and after the change.
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// The line without its `+`, `-` or space prefix.
    pub text: String,
}

/// Parses a unified diff into display lines, dropping the `diff --git`, `index`, `---` and `+++`
/// headers.
pub fn parse_unified_diff(patch: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    let (mut old_line, mut new_line) = (0u32, 0u32);
    let mut in_hunk = false;
    for line in patch.lines() {
        if let Some((old_start, new_start)) = line.strip_prefix("@@").and_then(parse_hunk_header) {
            old_line = old_start;
            new_line = new_start;
            in_hunk = true;
            lines.push(DiffLine {
                kind: DiffLineKind::Hunk,
                old_line: None,
                new_line: None,
                text: line.to_owned(),
            });
            continue;
        }
        if !in_hunk || line.starts_with("diff --git ") {
            in_hunk = false;
            if is_info_header(line) {
                lines.push(DiffLine {
                    kind: DiffLineKind::Info,
                    old_line: None,
                    new_line: None,
                    text: line.to_owned(),
                });
            }
            continue;
        }
        let (kind, text) = match line.as_bytes().first() {
            Some(b'+') => (DiffLineKind::Added, &line[1..]),
            Some(b'-') => (DiffLineKind::Removed, &line[1..]),
            Some(b' ') => (DiffLineKind::Context, &line[1..]),
            Some(b'\\') => (DiffLineKind::Info, line),
            None => (DiffLineKind::Context, ""),
            Some(_) => (DiffLineKind::Info, line),
        };
        let (old, new) = match kind {
            DiffLineKind::Added => {
                new_line += 1;
                (None, Some(new_line - 1))
            }
            DiffLineKind::Removed => {
                old_line += 1;
                (Some(old_line - 1), None)
            }
            DiffLineKind::Context => {
                old_line += 1;
                new_line += 1;
                (Some(old_line - 1), Some(new_line - 1))
            }
            DiffLineKind::Hunk | DiffLineKind::Info => (None, None),
        };
        lines.push(DiffLine {
            kind,
            old_line: old,
            new_line: new,
            text: text.to_owned(),
        });
    }
    lines
}

fn is_info_header(line: &str) -> bool {
    line.starts_with("Binary files ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("old mode ")
        || line.starts_with("new mode ")
}

/// The old and new start lines of ` -12,5 +14,6 @@ fn context`.
fn parse_hunk_header(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.trim_start().split(' ');
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let start = |range: &str| range.split(',').next()?.parse().ok();
    Some((start(old)?, start(new)?))
}

/// One side of a side-by-side row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideCell {
    /// The 1-based line number on this side.
    pub line: u32,
    pub text: String,
    /// Whether the line was removed (left) or added (right).
    pub changed: bool,
}

/// A row of the Before/After view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SideBySideRow {
    /// A hunk header or other line spanning both sides.
    Banner { kind: DiffLineKind, text: String },
    /// Lines shown next to each other; a side is empty where the other side has extra lines.
    Lines {
        left: Option<SideCell>,
        right: Option<SideCell>,
    },
}

/// Pairs a unified diff's lines for a Before/After view: context lines face each other, and in
/// each run of changes the removed lines face the added ones in order, the shorter side padded.
pub fn side_by_side(lines: &[DiffLine]) -> Vec<SideBySideRow> {
    let mut rows = Vec::new();
    let mut removed: Vec<SideCell> = Vec::new();
    let mut added: Vec<SideCell> = Vec::new();
    let flush =
        |rows: &mut Vec<SideBySideRow>, removed: &mut Vec<SideCell>, added: &mut Vec<SideCell>| {
            let count = removed.len().max(added.len());
            let mut left = removed.drain(..);
            let mut right = added.drain(..);
            for _ in 0..count {
                rows.push(SideBySideRow::Lines {
                    left: left.next(),
                    right: right.next(),
                });
            }
        };
    for line in lines {
        match line.kind {
            DiffLineKind::Removed => removed.push(SideCell {
                line: line.old_line.unwrap_or(0),
                text: line.text.clone(),
                changed: true,
            }),
            DiffLineKind::Added => added.push(SideCell {
                line: line.new_line.unwrap_or(0),
                text: line.text.clone(),
                changed: true,
            }),
            DiffLineKind::Context => {
                flush(&mut rows, &mut removed, &mut added);
                let cell = |number: Option<u32>| SideCell {
                    line: number.unwrap_or(0),
                    text: line.text.clone(),
                    changed: false,
                };
                rows.push(SideBySideRow::Lines {
                    left: Some(cell(line.old_line)),
                    right: Some(cell(line.new_line)),
                });
            }
            DiffLineKind::Hunk | DiffLineKind::Info => {
                flush(&mut rows, &mut removed, &mut added);
                rows.push(SideBySideRow::Banner {
                    kind: line.kind,
                    text: line.text.clone(),
                });
            }
        }
    }
    flush(&mut rows, &mut removed, &mut added);
    rows
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
