//! Warpium's git tools without UI: streaming `git log` with an incrementally laid out commit
//! graph, parsing `git blame --porcelain`, commit details, changed files and unified diffs, and
//! what rolling back a file runs. Everything here is protoc-free and unit-tested; the app only
//! renders the results.

pub mod blame;
pub mod diff;
pub mod format;
pub mod graph;
pub mod log;
pub mod oid;
pub mod process;
pub mod refs;
pub mod rollback;
pub mod show;
#[cfg(test)]
mod test_support;

pub use oid::Oid;
pub use process::{GitError, LogEvent, LogRow, LogStream, spawn_log};
