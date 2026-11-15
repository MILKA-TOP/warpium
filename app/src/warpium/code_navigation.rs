//! Go to Definition and Find Usages requests from a Kotlin or Java editor, answered by the code
//! index (see `code_index`).

#[cfg(not(target_family = "wasm"))]
use std::path::Path;
use std::path::PathBuf;
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
#[cfg(not(target_family = "wasm"))]
pub(crate) fn is_navigable_path(path: &Path) -> bool {
    code_index::Language::of_editable_path(path).is_some()
}

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "code_navigation_tests.rs"]
mod tests;
