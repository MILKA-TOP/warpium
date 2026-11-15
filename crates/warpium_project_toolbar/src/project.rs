//! Which project the active session belongs to, and where its toolbar configs live.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The project-level directory for Warpium's own config files.
pub const PROJECT_CONFIG_DIR: &str = ".warpium";
/// The toolbar config file name, in [`PROJECT_CONFIG_DIR`] and in the personal project directory.
pub const CONFIG_FILE_NAME: &str = "toolbar.toml";

/// How a project root was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectSource {
    /// The tab's pinned Project Explorer root.
    PinnedExplorerRoot,
    /// The nearest ancestor of the working directory with a `.warpium/` directory.
    WarpiumDirectory,
    /// The git repository containing the working directory.
    GitRepository,
}

/// The project the toolbar shows buttons for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContext {
    pub root: PathBuf,
    pub source: ProjectSource,
}

/// Finds the project for a tab, first match wins: the pinned Project Explorer root, the nearest
/// ancestor of `cwd` with a `.warpium/` directory, then the git repository containing `cwd`.
/// Paths should already be canonical; this only checks which directories exist.
pub fn detect_project(pinned_root: Option<&Path>, cwd: Option<&Path>) -> Option<ProjectContext> {
    if let Some(root) = pinned_root.filter(|root| root.is_dir()) {
        return Some(ProjectContext {
            root: root.to_path_buf(),
            source: ProjectSource::PinnedExplorerRoot,
        });
    }
    let cwd = cwd.filter(|cwd| cwd.is_dir())?;
    if let Some(root) = cwd
        .ancestors()
        .find(|dir| dir.join(PROJECT_CONFIG_DIR).is_dir())
    {
        return Some(ProjectContext {
            root: root.to_path_buf(),
            source: ProjectSource::WarpiumDirectory,
        });
    }
    // A worktree or submodule has a `.git` file rather than a directory.
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(|root| ProjectContext {
            root: root.to_path_buf(),
            source: ProjectSource::GitRepository,
        })
}

/// A stable, readable directory name for a project's personal files: the root's folder name and
/// a short hash of the full path, such as `kmp-bspb-demo-3f2a9c1b0d4e`.
pub fn project_id(root: &Path) -> String {
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut slug = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    slug = slug.trim_matches(|c| c == '-' || c == '.').to_owned();
    if slug.is_empty() {
        slug.push_str("project");
    }
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    format!("{slug}-{}", &hex::encode(digest)[..12])
}

/// The committed team config: `<root>/.warpium/toolbar.toml`.
pub fn team_config_path(root: &Path) -> PathBuf {
    root.join(PROJECT_CONFIG_DIR).join(CONFIG_FILE_NAME)
}

/// The personal overrides: `<warp_home>/projects/<project-id>/toolbar.toml`, where `warp_home` is
/// the app's home config directory (`~/.warp-oss`).
pub fn personal_config_path(warp_home: &Path, root: &Path) -> PathBuf {
    warp_home
        .join("projects")
        .join(project_id(root))
        .join(CONFIG_FILE_NAME)
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
