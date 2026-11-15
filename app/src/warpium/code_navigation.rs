//! Go to Definition and Find Usages requests from a Kotlin or Java editor, answered by the code
//! index (see `code_index`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Keymap context flag on a code editor whose file the code index can navigate, while
/// `warpium.code_index` is on.
pub(crate) const CODE_NAVIGATION_CONTEXT_FLAG: &str = "WarpiumCodeNavigation";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeNavigationKind {
    Definition,
    Usages,
}

#[derive(Clone, Debug)]
pub struct CodeNavigationRequest {
    pub kind: CodeNavigationKind,
    pub path: PathBuf,
    /// The editor's text, which may differ from the file on disk.
    pub text: Arc<str>,
    /// Zero-based line and character column of the name.
    pub line: u32,
    pub column: u32,
}

/// Kotlin sources and scripts, and Java sources.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn is_navigable_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("kt" | "kts" | "java")
    )
}

#[cfg(test)]
#[path = "code_navigation_tests.rs"]
mod tests;
