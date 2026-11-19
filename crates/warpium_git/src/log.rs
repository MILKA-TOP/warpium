//! `git log` arguments and an incremental parser for its output.

use crate::oid::Oid;
use crate::refs::{RefLabel, parse_decorations};

const FIELD_SEPARATOR: u8 = 0x1f;
const RECORD_SEPARATOR: u8 = 0x1e;

/// One record per commit: hash, parents, author name, email and time, decorations, subject.
pub const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub oid: Oid,
    pub parents: Vec<Oid>,
    pub author_name: String,
    pub author_email: String,
    /// Unix seconds.
    pub author_time: i64,
    pub refs: Vec<RefLabel>,
    pub subject: String,
}

/// Which history to list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Revisions {
    /// What `HEAD` can reach.
    #[default]
    Head,
    /// Every branch, remote branch and tag (`--all`).
    All,
    /// What one branch, tag or commit can reach.
    Rev(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogQuery {
    pub revisions: Revisions,
    /// `--author`: a substring of the author's name or email.
    pub author: Option<String>,
    /// `--grep`: a case-insensitive substring of the message.
    pub text: Option<String>,
    /// Paths relative to the repository root; only commits touching them are listed.
    pub paths: Vec<String>,
}

impl LogQuery {
    /// Whether the listed commits form a connected graph. Author and text filters drop commits
    /// without rewriting their children's parents, so their results are listed without one.
    pub fn shows_graph(&self) -> bool {
        self.author.is_none() && self.text.is_none()
    }

    /// The arguments after `git`.
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "log",
            "--date-order",
            "--decorate=full",
            "--no-color",
            "--encoding=UTF-8",
            LOG_FORMAT,
        ]
        .map(str::to_owned)
        .to_vec();
        if !self.paths.is_empty() {
            // Rewrites parents to the nearest listed ancestors, so a file's history stays one
            // connected graph.
            args.push("--parents".to_owned());
        }
        if let Some(author) = &self.author {
            args.push(format!("--author={author}"));
        }
        if let Some(text) = &self.text {
            args.push(format!("--grep={text}"));
        }
        if !self.shows_graph() {
            args.push("--regexp-ignore-case".to_owned());
            args.push("--fixed-strings".to_owned());
        }
        match &self.revisions {
            Revisions::Head => args.push("HEAD".to_owned()),
            Revisions::All => args.push("--all".to_owned()),
            Revisions::Rev(rev) => args.push(rev.clone()),
        }
        args.push("--".to_owned());
        args.extend(self.paths.iter().cloned());
        args
    }
}

/// Splits streamed `git log` output into commits, keeping a partial record until its end arrives.
#[derive(Default)]
pub struct LogParser {
    pending: Vec<u8>,
}

impl LogParser {
    pub fn feed(&mut self, data: &[u8], out: &mut Vec<Commit>) {
        let mut start = 0;
        for (index, byte) in data.iter().enumerate() {
            if *byte != RECORD_SEPARATOR {
                continue;
            }
            let record = &data[start..index];
            if self.pending.is_empty() {
                push_record(record, out);
            } else {
                self.pending.extend_from_slice(record);
                let pending = std::mem::take(&mut self.pending);
                push_record(&pending, out);
            }
            start = index + 1;
        }
        self.pending.extend_from_slice(&data[start..]);
    }

    /// Parses what is left once the output ended.
    pub fn finish(&mut self, out: &mut Vec<Commit>) {
        let pending = std::mem::take(&mut self.pending);
        push_record(&pending, out);
    }
}

fn push_record(record: &[u8], out: &mut Vec<Commit>) {
    let record = trim_ascii_whitespace_start(record);
    if record.is_empty() {
        return;
    }
    match parse_record(record) {
        Some(commit) => out.push(commit),
        None => log::warn!("[Git log] Skipped a malformed record"),
    }
}

fn trim_ascii_whitespace_start(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    &bytes[start..]
}

fn parse_record(record: &[u8]) -> Option<Commit> {
    let mut fields = record.splitn(7, |byte| *byte == FIELD_SEPARATOR);
    let oid = Oid::from_hex(std::str::from_utf8(fields.next()?).ok()?)?;
    let parents = std::str::from_utf8(fields.next()?)
        .ok()?
        .split_ascii_whitespace()
        .map(Oid::from_hex)
        .collect::<Option<Vec<_>>>()?;
    let author_name = lossy(fields.next()?);
    let author_email = lossy(fields.next()?);
    let author_time = std::str::from_utf8(fields.next()?)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let refs = parse_decorations(&lossy(fields.next()?));
    let subject = lossy(fields.next()?).trim_end().to_owned();
    Some(Commit {
        oid,
        parents,
        author_name,
        author_email,
        author_time,
        refs,
        subject,
    })
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod tests;
