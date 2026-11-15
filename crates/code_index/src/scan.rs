//! Finding the source files under an index root.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use ignore::{WalkBuilder, WalkState};

use crate::layout::{is_indexable, is_walkable_dir};

/// A source file found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FoundFile {
    pub rel_path: String,
    pub mtime_ns: u64,
    pub size: u64,
}

/// Walks `root` for Kotlin and Java sources, honoring `.gitignore` (also outside git
/// repositories) and skipping hidden and build directories, then walks the `build/generated`
/// directory of every Gradle module found on the way.
pub(crate) fn scan(root: &Path, threads: usize) -> Vec<FoundFile> {
    let found = Mutex::new(Vec::new());
    let module_dirs = Mutex::new(vec![root.to_path_buf()]);

    WalkBuilder::new(root)
        .hidden(true)
        .require_git(false)
        .threads(threads)
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_some_and(|file_type| file_type.is_dir())
                || entry.file_name().to_str().is_some_and(is_walkable_dir)
        })
        .build_parallel()
        .run(|| {
            let found = &found;
            let module_dirs = &module_dirs;
            Box::new(move |entry| {
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|file_type| file_type.is_file()) {
                    return WalkState::Continue;
                }
                let file_name = entry.file_name();
                if (file_name == "build.gradle" || file_name == "build.gradle.kts")
                    && let Some(parent) = entry.path().parent()
                    && parent != root
                {
                    lock(module_dirs).push(parent.to_path_buf());
                }
                if let Some(file) = found_file(root, entry.path(), entry.metadata().ok()) {
                    lock(found).push(file);
                }
                WalkState::Continue
            })
        });

    let mut found = found.into_inner().unwrap_or_else(|err| err.into_inner());
    let module_dirs = module_dirs.into_inner().unwrap_or_else(|err| err.into_inner());
    for module_dir in module_dirs {
        let generated = module_dir.join("build").join("generated");
        if generated.is_dir() {
            scan_generated(root, &generated, &mut found);
        }
    }
    found.sort_unstable_by(|a, b| a.rel_path.cmp(&b.rel_path));
    found.dedup_by(|a, b| a.rel_path == b.rel_path);
    found
}

/// Generated sources are usually gitignored, so this walk ignores ignore files.
fn scan_generated(root: &Path, generated: &Path, found: &mut Vec<FoundFile>) {
    let walker = WalkBuilder::new(generated)
        .standard_filters(false)
        .hidden(true)
        .build();
    for entry in walker.flatten() {
        if entry.file_type().is_some_and(|file_type| file_type.is_file())
            && let Some(file) = found_file(root, entry.path(), entry.metadata().ok())
        {
            found.push(file);
        }
    }
}

/// Stats `path` for the index, or `None` if it isn't an indexable file under `root`.
pub(crate) fn stat_file(root: &Path, path: &Path) -> Option<FoundFile> {
    found_file(root, path, std::fs::metadata(path).ok())
}

fn found_file(root: &Path, path: &Path, metadata: Option<std::fs::Metadata>) -> Option<FoundFile> {
    let rel_path = rel_path(root, path)?;
    if !is_indexable(&rel_path) {
        return None;
    }
    let metadata = metadata.filter(|metadata| metadata.is_file())?;
    let mtime_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or_default();
    Some(FoundFile {
        rel_path,
        mtime_ns,
        size: metadata.len(),
    })
}

/// `path` relative to `root` with `/` separators, or `None` if it's outside `root`.
pub(crate) fn rel_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

pub(crate) fn abs_path(root: &Path, rel_path: &str) -> PathBuf {
    rel_path.split('/').fold(root.to_path_buf(), |path, part| path.join(part))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
