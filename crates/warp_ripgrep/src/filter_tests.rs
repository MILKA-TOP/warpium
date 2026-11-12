use std::path::Path;

use super::*;

fn filter(include: &[&str], exclude: &[&str]) -> PathFilter {
    PathFilter::new(&SearchFilters {
        use_ignore_files: true,
        include_globs: include.iter().map(|s| s.to_string()).collect(),
        exclude_globs: exclude.iter().map(|s| s.to_string()).collect(),
    })
    .expect("globs should compile")
}

fn allowed(filter: &PathFilter, path: &str) -> bool {
    filter.is_file_allowed(Path::new(path))
}

#[test]
fn parse_glob_list_splits_on_commas_and_trims() {
    assert_eq!(
        parse_glob_list(" src/**/*.kt ,*.gradle.kts,, "),
        vec!["src/**/*.kt", "*.gradle.kts"]
    );
}

#[test]
fn parse_glob_list_keeps_commas_inside_braces() {
    assert_eq!(
        parse_glob_list("*.{kt,kts}, !build/**"),
        vec!["*.{kt,kts}", "!build/**"]
    );
}

#[test]
fn parse_glob_list_of_blank_input_is_empty() {
    assert!(parse_glob_list("  ").is_empty());
}

#[test]
fn empty_filter_allows_everything() {
    let filter = filter(&[], &[]);

    assert!(filter.is_empty());
    assert!(allowed(&filter, "a/b/c.rs"));
}

#[test]
fn exclude_without_slash_matches_any_segment_at_any_depth() {
    let filter = filter(&[], &["*build*"]);

    assert!(!allowed(&filter, "build/out.txt"));
    assert!(!allowed(&filter, "app/build/gen/R.kt"));
    assert!(!allowed(&filter, "app/rebuild-cache/x.bin"));
    assert!(!allowed(&filter, "src/build.gradle.kts"));
    assert!(allowed(&filter, "src/main.kt"));
}

#[test]
fn exclude_with_slash_is_anchored_to_root() {
    let filter = filter(&[], &["docs/*.md"]);

    assert!(!allowed(&filter, "docs/readme.md"));
    assert!(allowed(&filter, "app/docs/readme.md"));
    assert!(allowed(&filter, "docs/nested/readme.md"));
}

#[test]
fn leading_slash_anchors_pattern_without_other_slashes() {
    let filter = filter(&[], &["/target"]);

    assert!(!allowed(&filter, "target/debug/app"));
    assert!(allowed(&filter, "crates/foo/target/debug/app"));
}

#[test]
fn trailing_double_star_and_slash_match_directory_contents() {
    let filter = filter(&[], &["**/node_modules/**", "dist/"]);

    assert!(!allowed(&filter, "node_modules/pkg/index.js"));
    assert!(!allowed(&filter, "web/node_modules/pkg/index.js"));
    assert!(!allowed(&filter, "dist/bundle.js"));
    assert!(allowed(&filter, "web/src/index.js"));
}

#[test]
fn star_does_not_cross_directory_separator() {
    let filter = filter(&[], &["src/*.rs"]);

    assert!(!allowed(&filter, "src/lib.rs"));
    assert!(allowed(&filter, "src/nested/lib.rs"));
}

#[test]
fn negated_exclude_reincludes_matching_files() {
    let filter = filter(&[], &["*.lock", "!Cargo.lock"]);

    assert!(!allowed(&filter, "yarn.lock"));
    assert!(allowed(&filter, "Cargo.lock"));
    assert!(allowed(&filter, "crates/foo/Cargo.lock"));
}

#[test]
fn negated_exclude_cannot_reinclude_inside_excluded_directory() {
    let filter = filter(&[], &["build", "!keep.txt"]);

    assert!(!allowed(&filter, "build/keep.txt"));
    assert!(allowed(&filter, "src/keep.txt"));
}

#[test]
fn include_matches_files_by_glob() {
    let filter = filter(&["src/**/*.kt", "*.gradle.kts"], &[]);

    assert!(allowed(&filter, "src/main/App.kt"));
    assert!(allowed(&filter, "src/App.kt"));
    assert!(allowed(&filter, "app/build.gradle.kts"));
    assert!(!allowed(&filter, "src/main/App.java"));
    assert!(!allowed(&filter, "lib/src/App.kt"));
}

#[test]
fn include_of_directory_matches_its_contents() {
    let filter = filter(&["src"], &[]);

    assert!(allowed(&filter, "src/a/b.rs"));
    assert!(allowed(&filter, "app/src/b.rs"));
    assert!(!allowed(&filter, "tests/b.rs"));
}

#[test]
fn brace_alternation_matches_each_alternative() {
    let filter = filter(&["*.{kt,kts}"], &[]);

    assert!(allowed(&filter, "a/App.kt"));
    assert!(allowed(&filter, "build.gradle.kts"));
    assert!(!allowed(&filter, "App.java"));
}

#[test]
fn exclude_wins_over_include() {
    let filter = filter(&["*.kt"], &["*build*"]);

    assert!(allowed(&filter, "src/App.kt"));
    assert!(!allowed(&filter, "build/generated/App.kt"));
}

#[test]
fn negated_include_removes_paths_from_included_set() {
    let filter = filter(&["src", "!*Test.kt"], &[]);

    assert!(allowed(&filter, "src/App.kt"));
    assert!(!allowed(&filter, "src/AppTest.kt"));
}

#[test]
fn only_negated_includes_allow_everything_else() {
    let filter = filter(&["!*.md"], &[]);

    assert!(allowed(&filter, "src/App.kt"));
    assert!(!allowed(&filter, "README.md"));
}

#[test]
fn should_skip_dir_for_excluded_directory() {
    let filter = filter(&["*.kt"], &["**/node_modules"]);

    assert!(filter.should_skip_dir(Path::new("web/node_modules")));
    assert!(!filter.should_skip_dir(Path::new("web/src")));
}

#[test]
fn should_skip_dir_respects_negated_exclude() {
    let filter = filter(&[], &["*build*", "!buildSrc"]);

    assert!(filter.should_skip_dir(Path::new("app/build")));
    assert!(!filter.should_skip_dir(Path::new("buildSrc")));
}

#[test]
fn negation_after_leading_whitespace_still_negates() {
    let filter = filter(&[], &["*.lock", " !Cargo.lock"]);

    assert!(allowed(&filter, "Cargo.lock"));
}

#[test]
fn walked_file_check_matches_full_check_for_unpruned_paths() {
    let filter = filter(&["src", "!*Test.kt"], &["*.lock"]);

    assert!(filter.is_walked_file_allowed(Path::new("src/App.kt")));
    assert!(!filter.is_walked_file_allowed(Path::new("src/AppTest.kt")));
    assert!(!filter.is_walked_file_allowed(Path::new("src/yarn.lock")));
    assert!(!filter.is_walked_file_allowed(Path::new("docs/App.kt")));
}

#[test]
fn is_valid_glob_rejects_unclosed_alternation() {
    assert!(is_valid_glob("!**/node_modules"));
    assert!(!is_valid_glob("**/{node_modules"));
}

#[test]
fn invalid_glob_reports_the_offending_pattern() {
    let result = PathFilter::new(&SearchFilters {
        use_ignore_files: true,
        include_globs: vec!["src/[".to_string()],
        exclude_globs: Vec::new(),
    });

    let err = result.expect_err("unclosed character class should fail");
    assert_eq!(err.pattern, "src/[");
}

#[test]
fn default_filters_respect_ignore_files_without_globs() {
    let filters = SearchFilters::default();

    assert!(filters.use_ignore_files);
    assert!(!filters.has_globs());
}
