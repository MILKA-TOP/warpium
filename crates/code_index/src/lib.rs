//! Warpium's in-process declaration index for Kotlin and Java projects.
//!
//! Sources are parsed with the tree-sitter grammars the app already links (through `arborium`),
//! declarations are kept in memory for one-frame fuzzy lookups ("Go to Class/Symbol"), and a
//! compact snapshot in the user cache directory makes later starts fast. No Gradle, JVM or
//! language server is involved.

mod cache;
mod extract;
pub mod fuzzy;
mod index;
mod interner;
mod layout;
pub mod mcp;
mod model;
mod persist;
mod scan;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use cache::{default_snapshot_path, snapshot_path_in};
pub use fuzzy::MatchTier;
pub use index::{
    CodeIndex, IndexStats, PendingUpdate, SearchResult, SearchScope, SymbolInfo, UpdateStats,
    default_threads, prepare_changes, refresh_in_batches,
};
pub use interner::Sym;
pub use model::{Language, Origin, Position, SymbolKind, Visibility, flags};
pub use persist::{FORMAT_VERSION, PersistError};

/// How [`open`] got its index.
#[derive(Clone, Debug)]
pub struct OpenReport {
    /// Time to load the snapshot, if one was usable.
    pub loaded_in: Option<Duration>,
    /// Why the snapshot couldn't be used, if there was one.
    pub load_error: Option<String>,
    /// Time to walk the root and parse new or changed files (everything without a snapshot).
    pub refreshed_in: Duration,
    pub update: UpdateStats,
}

/// Loads the snapshot at `snapshot` (when usable) and brings it up to date with the files under
/// `root`, or builds the index from scratch. Doesn't write the snapshot.
pub fn open(root: &Path, snapshot: Option<&Path>, threads: usize) -> (CodeIndex, OpenReport) {
    let started = Instant::now();
    let (mut index, loaded_in, load_error) = match snapshot.map(|path| CodeIndex::load(path, root))
    {
        Some(Ok(index)) => (index, Some(started.elapsed()), None),
        Some(Err(PersistError::Io(err))) if err.kind() == std::io::ErrorKind::NotFound => {
            (CodeIndex::new(PathBuf::from(root)), None, None)
        }
        Some(Err(err)) => (CodeIndex::new(PathBuf::from(root)), None, Some(err.to_string())),
        None => (CodeIndex::new(PathBuf::from(root)), None, None),
    };
    let refresh_started = Instant::now();
    let update = index.refresh(threads);
    let report = OpenReport {
        loaded_in,
        load_error,
        refreshed_in: refresh_started.elapsed(),
        update,
    };
    (index, report)
}
