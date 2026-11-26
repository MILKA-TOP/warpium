use std::path::Path;

use super::*;

#[test]
fn plain_query_passes_through_with_the_regex_flag() {
    assert_eq!(
        search_pattern("foo.bar", false, false),
        ("foo.bar".to_owned(), false)
    );
    assert_eq!(
        search_pattern("foo.*", false, true),
        ("foo.*".to_owned(), true)
    );
}

#[test]
fn whole_words_escapes_the_query_and_forces_regex() {
    assert_eq!(
        search_pattern("foo.bar", true, false),
        ("\\bfoo\\.bar\\b".to_owned(), true)
    );
}

#[test]
fn whole_words_wraps_a_regex_query_in_a_non_capturing_group() {
    assert_eq!(
        search_pattern("foo|bar", true, true),
        ("\\b(?:foo|bar)\\b".to_owned(), true)
    );
}

#[test]
fn file_mask_parses_comma_separated_globs_keeping_brace_alternations() {
    let filters = search_filters("*.{kt,kts}, !*Test*, src/**", true);
    assert!(filters.use_ignore_files);
    assert_eq!(
        filters.include_globs,
        vec![
            "*.{kt,kts}".to_owned(),
            "!*Test*".to_owned(),
            "src/**".to_owned()
        ]
    );
    assert!(filters.exclude_globs.is_empty());
}

#[test]
fn empty_file_mask_searches_everything() {
    let filters = search_filters("  ", false);
    assert!(!filters.use_ignore_files);
    assert!(filters.include_globs.is_empty());
}

#[test]
fn home_and_filesystem_root_are_not_searchable() {
    let home = Path::new("/Users/dev");
    assert!(!is_searchable_root(Path::new("/"), Some(home)));
    assert!(!is_searchable_root(Path::new("/Users/dev"), Some(home)));
    assert!(is_searchable_root(
        Path::new("/Users/dev/project"),
        Some(home)
    ));
    assert!(is_searchable_root(Path::new("/Users/dev"), None));
}

#[test]
fn rows_list_each_file_header_followed_by_its_matches() {
    assert_eq!(
        flatten_rows(&[2, 0, 1]),
        vec![
            ResultRow::FileHeader { group: 0 },
            ResultRow::Match { group: 0, index: 0 },
            ResultRow::Match { group: 0, index: 1 },
            ResultRow::FileHeader { group: 1 },
            ResultRow::FileHeader { group: 2 },
            ResultRow::Match { group: 2, index: 0 },
        ]
    );
}

#[test]
fn summary_reports_progress_and_empty_states() {
    assert_eq!(results_summary(0, 0, 0, true), "Searching…");
    assert_eq!(results_summary(0, 0, 0, false), "No matches found");
}

#[test]
fn summary_counts_matches_and_files() {
    assert_eq!(results_summary(1, 1, 1, false), "1 match in 1 file");
    assert_eq!(results_summary(37, 4, 37, false), "37 matches in 4 files");
}

#[test]
fn summary_notes_when_the_list_is_capped() {
    assert_eq!(
        results_summary(2500, 40, 1000, false),
        "2500 matches in 40 files — showing first 1000"
    );
}
