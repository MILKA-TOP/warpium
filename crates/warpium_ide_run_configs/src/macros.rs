//! The IDE's path macros (`$PROJECT_DIR$`, `$MODULE_DIR$`, `$USER_HOME$`) and resolving the
//! paths they appear in.

use std::path::{Component, Path, PathBuf};

/// What the macros stand for in one configuration.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Macros<'a> {
    pub(crate) project_dir: &'a Path,
    /// The directory of the configuration's module, when it has one.
    pub(crate) module_dir: Option<&'a Path>,
    pub(crate) user_home: Option<&'a Path>,
}

impl Macros<'_> {
    /// `value` with the known macros replaced; unknown ones are left as written. `$MODULE_DIR$`
    /// without a module stands for the project directory, as in the IDE.
    pub(crate) fn expand(&self, value: &str) -> String {
        let project = self.project_dir.to_string_lossy();
        let module = self
            .module_dir
            .map_or_else(|| project.clone(), |dir| dir.to_string_lossy());
        let mut expanded = value
            .replace("$PROJECT_DIR$", &project)
            .replace("$ProjectFileDir$", &project)
            .replace("$MODULE_DIR$", &module)
            .replace("$MODULE_WORKING_DIR$", &module);
        if let Some(home) = self.user_home {
            expanded = expanded.replace("$USER_HOME$", &home.to_string_lossy());
        }
        expanded
    }

    /// A path option: macros expanded, relative paths taken from the project directory, and `.`
    /// and `..` resolved without touching the file system.
    pub(crate) fn path(&self, value: &str) -> PathBuf {
        let expanded = self.expand(value);
        let path = Path::new(&expanded);
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.project_dir.join(path)
        };
        normalize(&joined)
    }
}

/// `path` with `.` and `..` components resolved lexically.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}

/// `path` relative to `root`, with `..` when it is outside; `None` when it is `root` itself.
pub(crate) fn relative_to_root(root: &Path, path: &Path) -> Option<String> {
    let root = normalize(root);
    let path = normalize(path);
    if let Ok(inside) = path.strip_prefix(&root) {
        return (!inside.as_os_str().is_empty()).then(|| inside.to_string_lossy().into_owned());
    }
    let root_parts = root.components().collect::<Vec<_>>();
    let path_parts = path.components().collect::<Vec<_>>();
    let common = root_parts
        .iter()
        .zip(&path_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..root_parts.len() {
        relative.push("..");
    }
    for part in &path_parts[common..] {
        relative.push(part);
    }
    Some(relative.to_string_lossy().into_owned())
}

#[cfg(test)]
#[path = "macros_tests.rs"]
mod tests;
