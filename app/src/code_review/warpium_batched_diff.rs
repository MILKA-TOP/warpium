//! Batched diff loading: splitting one `git diff` invocation into per-file
//! patches, and synthesizing untracked files' all-additions diffs straight from
//! disk instead of spawning `git diff --no-index` per file.
//!
//! Design: `docs/warpium/research/code-review-panel.md` §4.9 F1 — the Code
//! Review panel loads all changed-file hunks with a single `git diff` process
//! (the GitHub Desktop pattern the upstream code already cites) instead of one
//! process per file.

use std::collections::HashMap;

use crate::code_review::diff_state::{DiffHunk, DiffLine, DiffLineType};

/// Splits the output of `git diff --no-ext-diff --patch-with-raw -z --no-color
/// <commit>` into per-file patch sections keyed by working-tree path.
///
/// `--patch-with-raw` prepends NUL-separated `:… M\0path\0` raw records whose
/// last record runs straight into the first `diff --git` header without a
/// newline, so each line is first cut at its last NUL. Sections then start at
/// `diff --git ` lines. A section's key is its `+++ b/<path>` line — or
/// `rename to` for pure renames and `--- a/<path>` for deletions, which have no
/// `+++` line; binary sections (no `+++` either) fall back to the `b/` side of
/// the `diff --git` header. `+++`/`---` paths may carry a trailing tab (git's
/// empty timestamp slot for paths with spaces) which is stripped.
pub(crate) fn split_combined_diff(output: &str) -> HashMap<String, String> {
    const SECTION_START: &str = "diff --git ";

    let mut sections = HashMap::new();
    // Candidate keys for the section being collected, in priority order.
    let mut plus_path: Option<String> = None;
    let mut rename_to: Option<String> = None;
    let mut minus_path: Option<String> = None;
    let mut header_new_path: Option<String> = None;
    let mut patch = String::new();

    let flush = |sections: &mut HashMap<String, String>,
                     plus_path: &mut Option<String>,
                     rename_to: &mut Option<String>,
                     minus_path: &mut Option<String>,
                     header_new_path: &mut Option<String>,
                     patch: &mut String| {
        let key = plus_path
            .take()
            .or_else(|| rename_to.take())
            .or_else(|| minus_path.take())
            .or_else(|| header_new_path.take())
            .filter(|path| !path.is_empty());
        if let Some(key) = key {
            sections.insert(key, std::mem::take(patch));
        } else {
            patch.clear();
        }
    };

    for line in output.lines() {
        // Cut the raw prelude off the first patch line (`…\0\0diff --git …`).
        let line = line.rsplit('\0').next().unwrap_or(line);

        if let Some(header) = line.strip_prefix(SECTION_START) {
            flush(
                &mut sections,
                &mut plus_path,
                &mut rename_to,
                &mut minus_path,
                &mut header_new_path,
                &mut patch,
            );
            // Best-effort fallback key: the b/ side of `a/<old> b/<new>`.
            header_new_path = header
                .rsplit_once(" b/")
                .map(|(_, new)| new.trim_end_matches('\t').to_owned())
                .filter(|path| !path.is_empty());
            patch.push_str(line);
            patch.push('\n');
            continue;
        }

        if let Some(new) = line.strip_prefix("+++ b/") {
            plus_path = Some(new.trim_end_matches(['\t', '\r']).to_owned());
        } else if let Some(old) = line.strip_prefix("--- a/") {
            minus_path = Some(old.trim_end_matches(['\t', '\r']).to_owned());
        } else if let Some(to) = line.strip_prefix("rename to ") {
            rename_to = Some(to.trim_end_matches('\r').to_owned());
        }
        if !patch.is_empty() {
            patch.push_str(line);
            patch.push('\n');
        }
    }
    flush(
        &mut sections,
        &mut plus_path,
        &mut rename_to,
        &mut minus_path,
        &mut header_new_path,
        &mut patch,
    );
    sections
}

/// Synthesizes the all-additions hunks of a new or untracked file from its
/// content, mirroring what `git diff --no-index /dev/null <file>` produces for
/// the panel's inline diff (one `@@ -0,0 +1,N @@` hunk).
pub(crate) fn additions_hunks(content: &str) -> Vec<DiffHunk> {
    let lines: Vec<DiffLine> = content
        .lines()
        .enumerate()
        .map(|(index, text)| DiffLine {
            line_type: DiffLineType::Add,
            old_line_number: None,
            new_line_number: Some(index + 1),
            text: text.to_owned(),
            no_trailing_newline: false,
        })
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let count = lines.len();
    vec![DiffHunk {
        old_start_line: 0,
        old_line_count: 0,
        new_start_line: 1,
        new_line_count: count,
        lines,
        unified_diff_start: 0,
        unified_diff_end: 0,
    }]
}

#[cfg(test)]
#[path = "warpium_batched_diff_tests.rs"]
mod tests;
