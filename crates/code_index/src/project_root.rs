//! Which folder to index so that a given file can be navigated (Go to Definition, Find Usages).

use std::path::{Path, PathBuf};

/// Why [`project_root_for_file`] chose its root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectRootKind {
    /// The outermost directory with Gradle settings (`settings.gradle(.kts)`).
    Gradle,
    /// A git repository (a `.git` directory or file).
    Repository,
    /// A folder the tab's project is locked to, holding neither of the above around the file.
    Locked,
}

/// The project root to index for `file`, with why it was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileProjectRoot {
    pub root: PathBuf,
    pub kind: ProjectRootKind,
}

/// Probes the file system for the markers [`project_root_for_file`] looks for.
pub trait RootMarkers {
    /// Whether `dir` holds `settings.gradle.kts` or `settings.gradle`.
    fn has_gradle_settings(&self, dir: &Path) -> bool;
    /// Whether `dir` holds `.git` (a directory, or a file for worktrees and submodules).
    fn is_repository(&self, dir: &Path) -> bool;
}

/// The real file system.
pub struct FsRootMarkers;

impl RootMarkers for FsRootMarkers {
    fn has_gradle_settings(&self, dir: &Path) -> bool {
        dir.join("settings.gradle.kts").is_file() || dir.join("settings.gradle").is_file()
    }

    fn is_repository(&self, dir: &Path) -> bool {
        dir.join(".git").exists()
    }
}

/// The folder to index so that `file` can be navigated. `locked_roots` are the folders the tab's
/// project is locked to (empty when it follows its terminals); paths are compared component-wise,
/// so they should be canonical like `file`.
///
/// When a locked folder holds the file (the deepest one, if they nest), the result stays inside
/// it: the user declared that folder the project, so nothing above it is indexed. Within it:
/// 1. the outermost directory with Gradle settings between the file and the locked folder
///    (inclusive), so a Gradle build is indexed whole even when a module of it is a nested
///    repository (a submodule) or the locked folder is a plain folder holding several builds;
/// 2. else the nearest repository in that range, so a folder of unrelated checkouts indexes the
///    one holding the file rather than all of them;
/// 3. else the locked folder itself, e.g. a plain folder of Kotlin files with no build files or
///    git, which no marker would find.
///
/// Otherwise (no locked folder holds the file), the outermost directory with Gradle settings
/// inside the file's repository (anywhere, outside one), else the repository; `None` when there
/// is neither.
pub fn project_root_for_file(
    file: &Path,
    locked_roots: &[PathBuf],
    markers: &impl RootMarkers,
) -> Option<FileProjectRoot> {
    let dirs = || file.ancestors().skip(1);
    let locked = locked_roots
        .iter()
        .filter(|root| file.starts_with(root) && file != root.as_path())
        .max_by_key(|root| root.components().count());
    if let Some(locked) = locked {
        let inside = || dirs().take_while(|dir| dir.starts_with(locked));
        let choice = if let Some(gradle) = inside()
            .filter(|dir| markers.has_gradle_settings(dir))
            .last()
        {
            FileProjectRoot {
                root: gradle.to_path_buf(),
                kind: ProjectRootKind::Gradle,
            }
        } else if let Some(repository) = inside().find(|dir| markers.is_repository(dir)) {
            FileProjectRoot {
                root: repository.to_path_buf(),
                kind: ProjectRootKind::Repository,
            }
        } else {
            FileProjectRoot {
                root: locked.clone(),
                kind: ProjectRootKind::Locked,
            }
        };
        return Some(choice);
    }

    let repository = dirs().find(|dir| markers.is_repository(dir));
    let gradle = dirs()
        .take_while(|dir| repository.is_none_or(|repository| dir.starts_with(repository)))
        .filter(|dir| markers.has_gradle_settings(dir))
        .last();
    match (gradle, repository) {
        (Some(root), _) => Some(FileProjectRoot {
            root: root.to_path_buf(),
            kind: ProjectRootKind::Gradle,
        }),
        // A repository is a project root even without build files at its top.
        (None, Some(repository)) => Some(FileProjectRoot {
            root: repository.to_path_buf(),
            kind: ProjectRootKind::Repository,
        }),
        (None, None) => None,
    }
}

#[cfg(test)]
#[path = "project_root_tests.rs"]
mod tests;
