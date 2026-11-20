//! A commit's full details (`git show -s`).

use crate::oid::Oid;
use crate::refs::{RefLabel, parse_decorations};

const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%D%x1f%B";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetails {
    pub oid: Oid,
    pub parents: Vec<Oid>,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_time: i64,
    pub refs: Vec<RefLabel>,
    /// The full message, without trailing blank lines.
    pub message: String,
}

pub fn details_args(oid: &Oid) -> Vec<String> {
    [
        "show",
        "-s",
        "--decorate=full",
        "--no-color",
        "--encoding=UTF-8",
        FORMAT,
        &oid.to_hex(),
    ]
    .map(str::to_owned)
    .to_vec()
}

pub fn parse_details(output: &str) -> Option<CommitDetails> {
    let mut fields = output.splitn(10, '\x1f');
    let oid = Oid::from_hex(fields.next()?.trim())?;
    let parents = fields
        .next()?
        .split_ascii_whitespace()
        .map(Oid::from_hex)
        .collect::<Option<Vec<_>>>()?;
    let author_name = fields.next()?.to_owned();
    let author_email = fields.next()?.to_owned();
    let author_time = fields.next()?.trim().parse().ok()?;
    let committer_name = fields.next()?.to_owned();
    let committer_email = fields.next()?.to_owned();
    let committer_time = fields.next()?.trim().parse().ok()?;
    let refs = parse_decorations(fields.next()?);
    let message = fields.next()?.trim_end().to_owned();
    Some(CommitDetails {
        oid,
        parents,
        author_name,
        author_email,
        author_time,
        committer_name,
        committer_email,
        committer_time,
        refs,
        message,
    })
}

#[cfg(test)]
#[path = "show_tests.rs"]
mod tests;
