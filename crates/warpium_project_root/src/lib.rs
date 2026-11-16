//! Warpium: a tab's project root.
//!
//! A tab either **follows** its terminals and editors (the Project Explorer shows the
//! repositories of their working directories, as upstream Warp does) or is **locked** to one or
//! more folders, which the explorer, the project toolbar, the code index, global search and code
//! review use regardless of where the tab's terminals are. A locked tab's terminals may be in an
//! unrelated folder, inside a container whose paths don't exist on this machine, or in an SSH
//! session; none of that re-roots the project.
//!
//! This crate holds the decision logic, free of the UI: the mode itself, how it is persisted with
//! the tab, which roots a locked tab shows and which terminal belongs to each, and whether the
//! active terminal is inside the project. The app supplies canonical paths and does the I/O.

use std::path::{Path, PathBuf};

/// A tab's project root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ProjectRoot {
    /// The explorer follows the tab's terminals and editors.
    #[default]
    Follow,
    /// The explorer shows these folders, whatever the terminals do.
    Locked(LockedRoots),
}

/// The folders of a locked project: never empty and without duplicates. The first one is the
/// primary root, which single-root consumers (the project toolbar, the code index) use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedRoots(Vec<PathBuf>);

impl LockedRoots {
    /// Returns `None` when `roots` holds no path. Duplicates and empty paths are dropped, keeping
    /// the first occurrence.
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Option<Self> {
        let mut unique: Vec<PathBuf> = Vec::new();
        for root in roots {
            if !root.as_os_str().is_empty() && !unique.contains(&root) {
                unique.push(root);
            }
        }
        (!unique.is_empty()).then_some(Self(unique))
    }

    pub fn primary(&self) -> &Path {
        &self.0[0]
    }

    pub fn as_slice(&self) -> &[PathBuf] {
        &self.0
    }
}

impl ProjectRoot {
    /// A project locked to a single folder.
    pub fn locked_to(root: PathBuf) -> Self {
        Self::locked([root])
    }

    /// A project locked to `roots`, or [`ProjectRoot::Follow`] when there are none.
    pub fn locked(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        LockedRoots::new(roots).map_or(Self::Follow, Self::Locked)
    }

    pub fn is_locked(&self) -> bool {
        matches!(self, Self::Locked(_))
    }

    /// The locked folders, primary first; empty while following.
    pub fn roots(&self) -> &[PathBuf] {
        match self {
            Self::Follow => &[],
            Self::Locked(roots) => roots.as_slice(),
        }
    }

    /// The primary locked folder, if locked.
    pub fn primary(&self) -> Option<&Path> {
        match self {
            Self::Follow => None,
            Self::Locked(roots) => Some(roots.primary()),
        }
    }

    /// Adds `root` to the project. A following tab becomes locked to `root` alone.
    pub fn with_root_added(&self, root: PathBuf) -> Self {
        Self::locked(self.roots().iter().cloned().chain([root]))
    }

    /// Removes `root` from the project. Removing the primary root promotes the next one; the
    /// last root can't be removed (unlock instead), so the project is returned unchanged.
    pub fn with_root_removed(&self, root: &Path) -> Self {
        match Self::locked(self.roots().iter().filter(|r| *r != root).cloned()) {
            Self::Follow => self.clone(),
            locked => locked,
        }
    }

    /// Keeps the folders `is_dir` accepts, such as the ones that still exist when a session is
    /// restored. A project whose folders are all gone follows its terminals again.
    pub fn retain_existing(&self, is_dir: impl Fn(&Path) -> bool) -> Self {
        Self::locked(self.roots().iter().filter(|r| is_dir(r)).cloned())
    }

    /// Where the active terminal is relative to the project, or `None` while following (every
    /// terminal belongs to a following project by definition).
    pub fn place_terminal(&self, cwd: &TerminalCwd) -> Option<TerminalPlacement> {
        let Self::Locked(roots) = self else {
            return None;
        };
        Some(match cwd {
            TerminalCwd::Unknown => TerminalPlacement::Unknown,
            TerminalCwd::NotOnHost => TerminalPlacement::NotOnHost,
            TerminalCwd::Host(path) => {
                if containing_root(roots.as_slice(), path).is_some() {
                    TerminalPlacement::InProject
                } else {
                    TerminalPlacement::OutsideProject
                }
            }
        })
    }
}

/// A terminal's working directory as far as the project is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalCwd {
    /// A directory that exists on this machine (canonical).
    Host(PathBuf),
    /// The terminal reports a directory that isn't on this machine: an SSH session, or a local
    /// session inside a container whose path doesn't exist on the host.
    NotOnHost,
    /// The terminal hasn't reported a directory yet.
    Unknown,
}

/// Where a locked project's active terminal is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalPlacement {
    InProject,
    /// In a directory on this machine outside every project root.
    OutsideProject,
    /// In a directory that isn't on this machine (SSH, a container).
    NotOnHost,
    /// Not known yet; nothing is shown.
    Unknown,
}

impl TerminalPlacement {
    /// The subtle hint shown in the explorer header, if any.
    pub fn indicator_text(self) -> Option<&'static str> {
        match self {
            Self::InProject | Self::Unknown => None,
            Self::OutsideProject => Some("Terminal is outside the project"),
            Self::NotOnHost => Some("Terminal is outside the project (not a local folder)"),
        }
    }
}

/// The deepest of `roots` that contains `path` (component-wise), if any.
pub fn containing_root<'a>(roots: &'a [PathBuf], path: &Path) -> Option<&'a Path> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .map(PathBuf::as_path)
}

/// The folder "Lock project to current folder" locks to: the explorer root holding the active
/// terminal's directory, else the explorer's most recent root, else the terminal's directory.
/// `explorer_roots` are local display roots, most recent first.
pub fn lock_target(explorer_roots: &[PathBuf], active_cwd: Option<&Path>) -> Option<PathBuf> {
    active_cwd
        .and_then(|cwd| containing_root(explorer_roots, cwd))
        .map(Path::to_path_buf)
        .or_else(|| explorer_roots.first().cloned())
        .or_else(|| active_cwd.map(Path::to_path_buf))
}

/// What a locked tab shows and which of its terminals belongs to each root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedLayout<T> {
    /// The explorer's roots, primary first. Terminal and editor paths never add roots.
    pub display_roots: Vec<PathBuf>,
    /// Each root that holds at least one terminal, with that terminal (the focused one when it is
    /// inside, else the first). Terminals outside the project aren't mapped.
    pub root_terminals: Vec<(PathBuf, T)>,
    /// The root of the focused terminal when it is inside the project, else the primary root.
    pub focused_root: PathBuf,
}

/// Lays out a locked tab given its terminals' working directories (`T` identifies a terminal).
pub fn locked_layout<T: Copy + PartialEq>(
    roots: &LockedRoots,
    terminal_cwds: &[(T, TerminalCwd)],
    focused_terminal: Option<T>,
) -> LockedLayout<T> {
    let root_of = |cwd: &TerminalCwd| match cwd {
        TerminalCwd::Host(path) => containing_root(roots.as_slice(), path),
        TerminalCwd::NotOnHost | TerminalCwd::Unknown => None,
    };

    let mut root_terminals: Vec<(PathBuf, T)> = Vec::new();
    for root in roots.as_slice() {
        let inside: Vec<T> = terminal_cwds
            .iter()
            .filter(|(_, cwd)| root_of(cwd) == Some(root.as_path()))
            .map(|(terminal, _)| *terminal)
            .collect();
        let chosen = focused_terminal
            .filter(|focused| inside.contains(focused))
            .or(inside.first().copied());
        if let Some(terminal) = chosen {
            root_terminals.push((root.clone(), terminal));
        }
    }

    let focused_root = focused_terminal
        .and_then(|focused| terminal_cwds.iter().find(|(t, _)| *t == focused))
        .and_then(|(_, cwd)| root_of(cwd))
        .unwrap_or(roots.primary())
        .to_path_buf();

    LockedLayout {
        display_roots: roots.as_slice().to_vec(),
        root_terminals,
        focused_root,
    }
}

/// A project root as stored with the tab. `primary` goes in the existing
/// `tabs.pinned_explorer_root` column (so sessions saved by older builds restore locked to their
/// pinned folder, and older builds still restore the primary root of newer sessions), and
/// `extra_roots` in `tabs.project_extra_roots` as a JSON array of strings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PersistedProjectRoot {
    pub primary: Option<PathBuf>,
    pub extra_roots: Option<String>,
}

impl ProjectRoot {
    /// The columns to save. Extra roots that aren't valid UTF-8 can't be written as JSON strings
    /// and are left out.
    pub fn to_persisted(&self) -> PersistedProjectRoot {
        let roots = self.roots();
        let extras: Vec<&str> = roots.iter().skip(1).filter_map(|r| r.to_str()).collect();
        PersistedProjectRoot {
            primary: roots.first().cloned(),
            extra_roots: (!extras.is_empty())
                .then(|| serde_json::to_string(&extras).unwrap_or_default()),
        }
    }

    /// Reads the saved columns. No primary root means the tab follows its terminals; unreadable
    /// extra roots are ignored.
    pub fn from_persisted(primary: Option<PathBuf>, extra_roots: Option<&str>) -> Self {
        let Some(primary) = primary else {
            return Self::Follow;
        };
        let extras: Vec<String> = extra_roots
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        Self::locked(
            [primary]
                .into_iter()
                .chain(extras.into_iter().map(PathBuf::from)),
        )
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
