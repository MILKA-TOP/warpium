use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::build_walker;
use crate::filter::SearchFilters;

/// Creates a git repository with ignored, hidden, build and nested files.
fn fixture() -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");
    let root = dir.path();
    for (path, contents) in [
        (".git/HEAD", "ref: refs/heads/master"),
        ("vendor/lib/.git", "gitdir: ../../.git/modules/lib"),
        ("vendor/lib/lib.c", "x"),
        (".gitignore", "ignored.txt\n/out/\n"),
        (".ignore", "scratch.md\n"),
        (".env", "SECRET=1"),
        (".config/settings.json", "{}"),
        ("ignored.txt", "x"),
        ("scratch.md", "x"),
        ("out/result.txt", "x"),
        ("README.md", "x"),
        ("Cargo.lock", "x"),
        ("yarn.lock", "x"),
        ("build.gradle.kts", "x"),
        ("build/generated/App.kt", "x"),
        ("node_modules/pkg/index.js", "x"),
        ("src/main/App.kt", "x"),
        ("src/main/AppTest.kt", "x"),
        ("src/main/Util.java", "x"),
        ("app/build/intermediates/R.kt", "x"),
        ("app/src/Feature.kt", "x"),
    ] {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    dir
}

fn walk(roots: &[PathBuf], filters: SearchFilters) -> Vec<String> {
    let mut files: Vec<String> = build_walker(roots, &filters)
        .expect("walker should build")
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
        .map(|entry| relative(entry.path(), roots))
        .collect();
    files.sort();
    files
}

fn relative(path: &Path, roots: &[PathBuf]) -> String {
    let root = roots
        .iter()
        .find(|root| path.starts_with(root))
        .expect("entry under a root");
    let prefix = root.file_name().unwrap().to_string_lossy();
    let rest = path
        .strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    format!("{prefix}:{rest}")
}

fn globs(patterns: &[&str]) -> Vec<String> {
    patterns.iter().map(|s| s.to_string()).collect()
}

fn root_name(dir: &TempDir) -> String {
    dir.path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn with_root(dir: &TempDir, files: &[&str]) -> Vec<String> {
    let name = root_name(dir);
    files.iter().map(|f| format!("{name}:{f}")).collect()
}

#[test]
fn default_filters_respect_ignore_files_and_skip_hidden() {
    let dir = fixture();

    let files = walk(&[dir.path().to_path_buf()], SearchFilters::default());

    assert_eq!(
        files,
        with_root(
            &dir,
            &[
                "Cargo.lock",
                "README.md",
                "app/build/intermediates/R.kt",
                "app/src/Feature.kt",
                "build.gradle.kts",
                "build/generated/App.kt",
                "node_modules/pkg/index.js",
                "src/main/App.kt",
                "src/main/AppTest.kt",
                "src/main/Util.java",
                "vendor/lib/lib.c",
                "yarn.lock",
            ]
        )
    );
}

#[test]
fn disabling_ignore_files_includes_ignored_and_hidden_but_not_git_dir() {
    let dir = fixture();

    let files = walk(
        &[dir.path().to_path_buf()],
        SearchFilters {
            use_ignore_files: false,
            ..Default::default()
        },
    );

    assert_eq!(
        files,
        with_root(
            &dir,
            &[
                ".config/settings.json",
                ".env",
                ".gitignore",
                ".ignore",
                "Cargo.lock",
                "README.md",
                "app/build/intermediates/R.kt",
                "app/src/Feature.kt",
                "build.gradle.kts",
                "build/generated/App.kt",
                "ignored.txt",
                "node_modules/pkg/index.js",
                "out/result.txt",
                "scratch.md",
                "src/main/App.kt",
                "src/main/AppTest.kt",
                "src/main/Util.java",
                "vendor/lib/lib.c",
                "yarn.lock",
            ]
        )
    );
}

#[test]
fn exclude_globs_prune_matching_segments_at_any_depth() {
    let dir = fixture();

    let files = walk(
        &[dir.path().to_path_buf()],
        SearchFilters {
            exclude_globs: globs(&["*build*", "**/node_modules/**", "*.lock"]),
            ..Default::default()
        },
    );

    assert_eq!(
        files,
        with_root(
            &dir,
            &[
                "README.md",
                "app/src/Feature.kt",
                "src/main/App.kt",
                "src/main/AppTest.kt",
                "src/main/Util.java",
                "vendor/lib/lib.c",
            ]
        )
    );
}

#[test]
fn include_globs_select_matching_files_only() {
    let dir = fixture();

    let files = walk(
        &[dir.path().to_path_buf()],
        SearchFilters {
            include_globs: globs(&["src/**/*.kt", "*.gradle.kts"]),
            ..Default::default()
        },
    );

    assert_eq!(
        files,
        with_root(
            &dir,
            &["build.gradle.kts", "src/main/App.kt", "src/main/AppTest.kt"]
        )
    );
}

#[test]
fn include_and_exclude_combine_as_intersection() {
    let dir = fixture();

    let files = walk(
        &[dir.path().to_path_buf()],
        SearchFilters {
            include_globs: globs(&["*.kt"]),
            exclude_globs: globs(&["*build*", "*Test.kt"]),
            ..Default::default()
        },
    );

    assert_eq!(
        files,
        with_root(&dir, &["app/src/Feature.kt", "src/main/App.kt"])
    );
}

#[test]
fn negated_exclude_reincludes_file() {
    let dir = fixture();

    let files = walk(
        &[dir.path().to_path_buf()],
        SearchFilters {
            include_globs: globs(&["*.lock"]),
            exclude_globs: globs(&["*.lock", "!Cargo.lock"]),
            ..Default::default()
        },
    );

    assert_eq!(files, with_root(&dir, &["Cargo.lock"]));
}

#[test]
fn globs_apply_with_ignore_files_disabled() {
    let dir = fixture();

    let files = walk(
        &[dir.path().to_path_buf()],
        SearchFilters {
            use_ignore_files: false,
            include_globs: globs(&["*.txt", "*.json"]),
            exclude_globs: globs(&["/out"]),
        },
    );

    assert_eq!(
        files,
        with_root(&dir, &[".config/settings.json", "ignored.txt"])
    );
}

#[test]
fn anchored_globs_are_relative_to_each_root() {
    let first = fixture();
    let second = fixture();
    let roots = [first.path().to_path_buf(), second.path().to_path_buf()];

    let files = walk(
        &roots,
        SearchFilters {
            include_globs: globs(&["src/main/App.kt"]),
            ..Default::default()
        },
    );

    let mut expected = with_root(&first, &["src/main/App.kt"]);
    expected.extend(with_root(&second, &["src/main/App.kt"]));
    expected.sort();
    assert_eq!(files, expected);
}

#[test]
fn invalid_glob_fails_to_build_walker() {
    let dir = fixture();

    let result = build_walker(
        &[dir.path().to_path_buf()],
        &SearchFilters {
            exclude_globs: globs(&["{unclosed"]),
            ..Default::default()
        },
    );

    assert!(result.is_err());
}
