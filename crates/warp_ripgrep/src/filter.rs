//! Path filtering for global search: ignore-file handling plus VS Code style include/exclude
//! globs.
//!
//! Glob semantics, applied to paths relative to the search root they were found under:
//! - A pattern without a `/` (ignoring a trailing one) matches at any depth, so `*build*` matches
//!   any path segment containing "build". A pattern with a `/` is anchored to the search root; a
//!   leading `/` or `./` only marks it as anchored.
//! - `*` and `?` never cross a `/`; `**` does.
//! - A pattern matching a directory also matches everything below it.
//! - A leading `!` negates a pattern. In the exclude list it re-includes paths, but (as in
//!   gitignore) never a path whose parent directory is excluded. In the include list it removes
//!   paths from the included set.
//! - A path is searched when it is included (or no positive include pattern was given), not
//!   removed by a negated include, and not excluded.

use std::path::Path;

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

/// User-facing options controlling which files a search visits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchFilters {
    /// Whether `.gitignore`, `.ignore`, git excludes and the global gitignore are respected and
    /// hidden files are skipped. When false, everything is searched except `.git` directories.
    pub use_ignore_files: bool,
    /// Glob patterns a file must match to be searched.
    pub include_globs: Vec<String>,
    /// Glob patterns for files and directories to skip.
    pub exclude_globs: Vec<String>,
}

impl Default for SearchFilters {
    fn default() -> Self {
        Self {
            use_ignore_files: true,
            include_globs: Vec::new(),
            exclude_globs: Vec::new(),
        }
    }
}

impl SearchFilters {
    /// Whether any include or exclude glob is set.
    pub fn has_globs(&self) -> bool {
        !self.include_globs.is_empty() || !self.exclude_globs.is_empty()
    }
}

/// Splits a comma-separated list of globs, trimming whitespace and dropping empty entries.
/// Commas inside `{...}` alternations are kept, so `*.{kt,kts}` stays a single pattern.
pub fn parse_glob_list(input: &str) -> Vec<String> {
    let mut patterns = Vec::new();
    let mut current = String::new();
    let mut brace_depth = 0usize;
    for ch in input.chars() {
        match ch {
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if brace_depth == 0 => {
                push_trimmed(&mut patterns, &current);
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    push_trimmed(&mut patterns, &current);
    patterns
}

fn push_trimmed(patterns: &mut Vec<String>, pattern: &str) {
    let trimmed = pattern.trim();
    if !trimmed.is_empty() {
        patterns.push(trimmed.to_owned());
    }
}

/// Error returned when a glob pattern cannot be compiled.
#[derive(Debug, thiserror::Error)]
#[error("Invalid glob pattern \"{pattern}\": {source}")]
pub struct InvalidGlobError {
    pub pattern: String,
    #[source]
    source: globset::Error,
}

/// Compiled include/exclude globs.
#[derive(Clone, Debug)]
pub struct PathFilter {
    include: GlobSet,
    negated_include: GlobSet,
    exclude: GlobSet,
    negated_exclude: GlobSet,
}

impl PathFilter {
    /// Compiles the include and exclude globs of `filters`.
    pub fn new(filters: &SearchFilters) -> Result<Self, InvalidGlobError> {
        let (include, negated_include) = build_glob_sets(&filters.include_globs)?;
        let (exclude, negated_exclude) = build_glob_sets(&filters.exclude_globs)?;
        Ok(Self {
            include,
            negated_include,
            exclude,
            negated_exclude,
        })
    }

    /// Whether this filter accepts every path.
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.negated_include.is_empty() && self.exclude.is_empty()
    }

    /// Whether the file at `relative_path` (relative to its search root) should be searched.
    pub fn is_file_allowed(&self, relative_path: &Path) -> bool {
        let mut any_included = self.include.is_empty();
        for path in relative_path
            .ancestors()
            .filter(|p| !p.as_os_str().is_empty())
        {
            if self.is_directly_excluded(path) || self.negated_include.is_match(path) {
                return false;
            }
            any_included |= self.include.is_match(path);
        }
        any_included
    }

    /// Like [`Self::is_file_allowed`], but assumes no ancestor directory was rejected by
    /// [`Self::should_skip_dir`], as during a top-down walk, so only the include check needs to
    /// look at ancestors.
    pub fn is_walked_file_allowed(&self, relative_path: &Path) -> bool {
        if self.should_skip_dir(relative_path) {
            return false;
        }
        self.include.is_empty()
            || relative_path
                .ancestors()
                .filter(|p| !p.as_os_str().is_empty())
                .any(|p| self.include.is_match(p))
    }

    /// Whether the directory at `relative_path` and everything under it can be skipped. Assumes
    /// none of its ancestors were skipped, as during a top-down walk.
    pub fn should_skip_dir(&self, relative_path: &Path) -> bool {
        self.is_directly_excluded(relative_path) || self.negated_include.is_match(relative_path)
    }

    /// Whether `path` itself (not considering its ancestors) is excluded.
    fn is_directly_excluded(&self, path: &Path) -> bool {
        self.exclude.is_match(path) && !self.negated_exclude.is_match(path)
    }
}

/// Builds the positive and negated glob sets for one comma-separated list.
fn build_glob_sets(patterns: &[String]) -> Result<(GlobSet, GlobSet), InvalidGlobError> {
    let mut positive = GlobSetBuilder::new();
    let mut negated = GlobSetBuilder::new();
    for raw in patterns {
        let trimmed = raw.trim();
        let (is_negated, pattern) = match trimmed.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, trimmed),
        };
        let Some(normalized) = normalize_pattern(pattern) else {
            continue;
        };
        let glob = compile_glob(&normalized).map_err(|source| InvalidGlobError {
            pattern: raw.clone(),
            source,
        })?;
        if is_negated {
            negated.add(glob);
        } else {
            positive.add(glob);
        }
    }
    let build = |builder: GlobSetBuilder| {
        builder.build().map_err(|source| InvalidGlobError {
            pattern: patterns.join(", "),
            source,
        })
    };
    Ok((build(positive)?, build(negated)?))
}

/// Whether `pattern` (optionally `!`-negated) compiles as a search glob.
pub fn is_valid_glob(pattern: &str) -> bool {
    build_glob_sets(std::slice::from_ref(&pattern.to_owned())).is_ok()
}

/// Converts a user pattern into a glob matched against root-relative paths. Returns `None` for
/// patterns that match nothing meaningful (e.g. a bare `/`).
fn normalize_pattern(pattern: &str) -> Option<String> {
    let mut pattern = pattern.trim();
    pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    let anchored = pattern.starts_with('/');
    pattern = pattern.trim_start_matches('/');
    // Matching a directory already covers its contents, so `dir/` and `dir/**` reduce to `dir`.
    loop {
        let before = pattern;
        pattern = pattern.trim_end_matches('/');
        pattern = pattern.strip_suffix("/**").unwrap_or(pattern);
        if pattern == before {
            break;
        }
    }
    if pattern.is_empty() {
        return None;
    }
    if !anchored && !pattern.contains('/') && pattern != "**" {
        Some(format!("**/{pattern}"))
    } else {
        Some(pattern.to_owned())
    }
}

fn compile_glob(pattern: &str) -> Result<Glob, globset::Error> {
    GlobBuilder::new(pattern).literal_separator(true).build()
}

#[cfg(test)]
#[path = "filter_tests.rs"]
mod tests;
