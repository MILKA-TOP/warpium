//! `git blame --porcelain` parsing.

use std::collections::HashMap;

use crate::oid::Oid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameCommit {
    pub oid: Oid,
    pub author: String,
    pub author_mail: String,
    /// Unix seconds.
    pub author_time: i64,
    pub summary: String,
    /// The file's path in that commit, which differs from the current one after a rename.
    pub filename: String,
}

impl BlameCommit {
    /// Lines git attributes to the working tree (or to `--contents`): not committed yet.
    pub fn is_uncommitted(&self) -> bool {
        self.oid.is_zero()
    }
}

/// The commit of every line of a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blame {
    commits: Vec<BlameCommit>,
    /// For each line (0-based), an index into `commits`.
    lines: Vec<u32>,
}

impl Blame {
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The commit of a 0-based line.
    pub fn line(&self, line: usize) -> Option<&BlameCommit> {
        self.lines
            .get(line)
            .and_then(|index| self.commits.get(*index as usize))
    }

    /// An index identifying the line's commit, the same for every line of that commit.
    pub fn commit_index(&self, line: usize) -> Option<usize> {
        self.lines.get(line).map(|index| *index as usize)
    }

    pub fn commits(&self) -> &[BlameCommit] {
        &self.commits
    }

    /// The widest author name, in characters, for sizing the annotation column.
    pub fn max_author_chars(&self) -> usize {
        self.commits
            .iter()
            .map(|commit| commit.author.chars().count())
            .max()
            .unwrap_or(0)
    }

    /// Parses `git blame --porcelain` output.
    pub fn parse_porcelain(output: &str) -> Result<Self, String> {
        let mut commits: Vec<BlameCommit> = Vec::new();
        let mut index_by_oid: HashMap<Oid, u32> = HashMap::new();
        let mut lines: Vec<u32> = Vec::new();
        let mut current: Option<(u32, usize)> = None;
        for line in output.lines() {
            if line.starts_with('\t') {
                let Some((commit, final_line)) = current.take() else {
                    return Err("content line without a header".to_owned());
                };
                if lines.len() < final_line {
                    lines.resize(final_line, commit);
                }
                lines[final_line - 1] = commit;
                continue;
            }
            if current.is_none() {
                let mut fields = line.split(' ');
                let oid = fields
                    .next()
                    .and_then(Oid::from_hex)
                    .ok_or_else(|| format!("bad header: {line}"))?;
                let _original_line = fields.next();
                let final_line: usize = fields
                    .next()
                    .and_then(|field| field.parse().ok())
                    .filter(|line| *line > 0)
                    .ok_or_else(|| format!("bad header: {line}"))?;
                let index = *index_by_oid.entry(oid).or_insert_with(|| {
                    commits.push(BlameCommit {
                        oid,
                        author: String::new(),
                        author_mail: String::new(),
                        author_time: 0,
                        summary: String::new(),
                        filename: String::new(),
                    });
                    (commits.len() - 1) as u32
                });
                current = Some((index, final_line));
                continue;
            }
            let Some((commit, _)) = current else {
                continue;
            };
            let commit = &mut commits[commit as usize];
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "author" => commit.author = value.to_owned(),
                "author-mail" => {
                    commit.author_mail = value
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_owned()
                }
                "author-time" => commit.author_time = value.parse().unwrap_or(0),
                "summary" => commit.summary = value.to_owned(),
                "filename" => commit.filename = value.to_owned(),
                _ => {}
            }
        }
        Ok(Self { commits, lines })
    }
}

/// Arguments of `git blame` for `path` (relative to the repository root). With `from_stdin`, the
/// file's contents are read from stdin, so unsaved edits show as not committed yet.
pub fn blame_args(path: &str, from_stdin: bool) -> Vec<String> {
    let mut args: Vec<String> = ["blame", "--porcelain", "--encoding=UTF-8"]
        .map(str::to_owned)
        .to_vec();
    if from_stdin {
        args.push("--contents".to_owned());
        args.push("-".to_owned());
    }
    args.push("--".to_owned());
    args.push(path.to_owned());
    args
}

#[cfg(test)]
#[path = "blame_tests.rs"]
mod tests;
