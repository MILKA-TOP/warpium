use std::fs;

use tempfile::TempDir;

use super::*;
use crate::model::flags;

/// Copies `fixtures/<name>` into a temporary directory, so tests can edit it.
fn fixture(name: &str) -> TempDir {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name);
    let dir = TempDir::new().expect("temp dir");
    copy_dir(&source, dir.path());
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create dir");
    for entry in fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

fn write(root: &Path, rel_path: &str, contents: &str) -> PathBuf {
    let path = root.join(rel_path);
    fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
    fs::write(&path, contents).expect("write file");
    path
}

fn names(results: &[SearchResult]) -> Vec<&str> {
    results
        .iter()
        .map(|result| result.symbol.name.as_str())
        .collect()
}

#[test]
fn indexes_every_kmp_source_set_and_generated_sources() {
    let dir = fixture("kmp");

    let index = CodeIndex::build(dir.path(), 2);

    let mut source_sets: Vec<(String, Origin)> = index
        .live_files()
        .map(|file| {
            (
                index.interner.resolve(file.source_set).to_owned(),
                file.origin,
            )
        })
        .collect();
    source_sets.sort();
    assert_eq!(
        source_sets,
        vec![
            ("androidMain".to_owned(), Origin::Main),
            ("commonMain".to_owned(), Origin::Main),
            ("commonMain".to_owned(), Origin::Generated),
            ("commonTest".to_owned(), Origin::Test),
            ("iosMain".to_owned(), Origin::Main),
        ]
    );
}

#[test]
fn links_expect_declarations_to_their_actuals() {
    let dir = fixture("kmp");
    let index = CodeIndex::build(dir.path(), 2);
    let expect = index
        .search("Platform", SearchScope::Types, 10)
        .into_iter()
        .map(|result| result.symbol)
        .find(|symbol| symbol.has_flag(flags::EXPECT))
        .expect("expect class is indexed");

    let counterparts = index.expect_actual_counterparts(&expect);

    assert_eq!(
        counterparts
            .iter()
            .map(|symbol| (symbol.source_set.as_str(), symbol.has_flag(flags::ACTUAL)))
            .collect::<Vec<_>>(),
        vec![("androidMain", true), ("iosMain", true)]
    );
}

#[test]
fn ranks_exact_then_prefix_then_camel_then_word_start_then_fuzzy() {
    let dir = TempDir::new().expect("temp dir");
    write(
        dir.path(),
        "src/main/kotlin/Names.kt",
        "class Duvms\nclass SubUserViewModel\nclass UserViewModel\nclass UvmHolder\nclass Uvm\n",
    );
    let index = CodeIndex::build(dir.path(), 1);

    let results = index.search("uvm", SearchScope::Types, 10);

    assert_eq!(
        names(&results),
        vec![
            "Uvm",
            "UvmHolder",
            "UserViewModel",
            "SubUserViewModel",
            "Duvms"
        ]
    );
    assert_eq!(
        results.iter().map(|result| result.tier).collect::<Vec<_>>(),
        vec![
            MatchTier::Exact,
            MatchTier::Prefix,
            MatchTier::Camel,
            MatchTier::WordStart,
            MatchTier::Fuzzy,
        ]
    );
}

#[test]
fn ranks_project_code_above_tests_and_generated_code_within_a_tier() {
    let dir = fixture("kmp");
    let index = CodeIndex::build(dir.path(), 2);

    let results = index.search("UserViewModel", SearchScope::Types, 10);

    assert_eq!(
        results
            .iter()
            .map(|result| (result.symbol.name.as_str(), result.symbol.origin))
            .collect::<Vec<_>>(),
        vec![
            ("UserViewModel", Origin::Main),
            ("UserViewModelTest", Origin::Test),
            ("UserViewModelFactory", Origin::Generated),
        ]
    );
}

#[test]
fn camel_hump_query_finds_the_class() {
    let dir = fixture("plain");
    let index = CodeIndex::build(dir.path(), 2);

    let results = index.search("UVM", SearchScope::Types, 1);

    assert_eq!(results[0].symbol.qualified_name(), "org.acme.UserViewModel");
    assert_eq!(results[0].matched_indices, vec![0, 4, 8]);
}

#[test]
fn type_scope_leaves_out_members() {
    let dir = fixture("plain");
    let index = CodeIndex::build(dir.path(), 2);

    let results = index.search("findUser", SearchScope::Types, 10);

    assert_eq!(names(&results), Vec::<&str>::new());
}

#[test]
fn symbol_scope_finds_members_and_java_declarations() {
    let dir = fixture("plain");
    let index = CodeIndex::build(dir.path(), 2);

    let find_user = index.search("findUser", SearchScope::All, 10);
    let max_users = index.search("MAX_USERS", SearchScope::All, 1);

    assert_eq!(
        find_user
            .iter()
            .map(|result| result.symbol.container.as_str())
            .collect::<Vec<_>>(),
        vec!["UserRepository", "InMemoryUserRepository"]
    );
    assert_eq!(
        max_users[0].symbol.qualified_name(),
        "org.acme.legacy.LegacyUserService.MAX_USERS"
    );
    assert_eq!(max_users[0].symbol.language, Language::Java);
}

#[test]
fn qualified_query_narrows_by_package_or_container() {
    let dir = fixture("plain");
    let index = CodeIndex::build(dir.path(), 2);

    let results = index.search("legacy.Cache", SearchScope::Types, 10);
    let none = index.search("compose.Cache", SearchScope::Types, 10);

    assert_eq!(names(&results), vec!["Cache"]);
    assert_eq!(names(&none), Vec::<&str>::new());
}

#[test]
fn receiver_qualified_query_finds_extension_functions() {
    let dir = fixture("plain");
    let index = CodeIndex::build(dir.path(), 2);

    let results = index.search("User.ini", SearchScope::All, 10);

    assert_eq!(names(&results), vec!["initials"]);
    assert_eq!(results[0].symbol.receiver.as_deref(), Some("User"));
}

#[test]
fn qualifier_alone_lists_members_and_extensions() {
    let dir = fixture("plain");
    let index = CodeIndex::build(dir.path(), 2);

    let results = index.search("LoadState.", SearchScope::All, 10);

    assert_eq!(names(&results), vec!["user", "Loaded", "Loading"]);
}

#[test]
fn empty_query_finds_nothing() {
    let dir = fixture("plain");
    let index = CodeIndex::build(dir.path(), 2);

    assert_eq!(
        names(&index.search(" ", SearchScope::All, 10)),
        Vec::<&str>::new()
    );
}

#[test]
fn honors_gitignore_outside_git_repositories() {
    let dir = fixture("plain");
    write(dir.path(), ".gitignore", "ignored/\n");
    write(
        dir.path(),
        "ignored/src/main/kotlin/Hidden.kt",
        "class Hidden\n",
    );

    let index = CodeIndex::build(dir.path(), 2);

    assert_eq!(
        names(&index.search("Hidden", SearchScope::Types, 10)),
        Vec::<&str>::new()
    );
}

#[test]
fn indexes_generated_sources_even_when_gitignored() {
    let dir = fixture("kmp");
    write(dir.path(), ".gitignore", "build/\n");

    let index = CodeIndex::build(dir.path(), 2);

    assert_eq!(
        names(&index.search("UserViewModelFactory", SearchScope::Types, 10)),
        vec!["UserViewModelFactory"]
    );
}

#[test]
fn apply_changes_reparses_an_edited_file() {
    let dir = fixture("plain");
    let mut index = CodeIndex::build(dir.path(), 2);
    let path = write(
        dir.path(),
        "src/main/kotlin/org/acme/Users.kt",
        "package org.acme\nclass RenamedRepository\n",
    );

    let stats = index.apply_changes(&[path], 1);

    assert_eq!(stats.files_parsed, 1);
    assert_eq!(
        names(&index.search("RenamedRepository", SearchScope::Types, 10)),
        vec!["RenamedRepository"]
    );
    assert_eq!(
        names(&index.search("UserRepository", SearchScope::Types, 10)),
        Vec::<&str>::new()
    );
}

#[test]
fn apply_changes_drops_a_deleted_file() {
    let dir = fixture("plain");
    let mut index = CodeIndex::build(dir.path(), 2);
    let path = dir
        .path()
        .join("src/test/kotlin/org/acme/UserViewModelTest.kt");
    fs::remove_file(&path).expect("delete file");

    let stats = index.apply_changes(&[path], 1);

    assert_eq!(stats.files_removed, 1);
    assert_eq!(
        names(&index.search("UserViewModelTest", SearchScope::Types, 10)),
        Vec::<&str>::new()
    );
}

#[test]
fn apply_changes_drops_every_file_of_a_deleted_directory() {
    let dir = fixture("plain");
    let mut index = CodeIndex::build(dir.path(), 2);
    let java_dir = dir.path().join("src/main/java");
    fs::remove_dir_all(&java_dir).expect("delete dir");

    let stats = index.apply_changes(&[java_dir], 1);

    assert_eq!(stats.files_removed, 1);
    assert_eq!(index.stats().files, 2);
}

#[test]
fn apply_changes_indexes_an_added_directory() {
    let dir = fixture("plain");
    let mut index = CodeIndex::build(dir.path(), 2);
    write(
        dir.path(),
        "feature/src/main/kotlin/Feature.kt",
        "class FeatureScreen\n",
    );

    index.apply_changes(&[dir.path().join("feature")], 1);

    let results = index.search("FeatureScreen", SearchScope::Types, 1);
    assert_eq!(results[0].symbol.module, "feature");
}

#[test]
fn apply_changes_ignores_files_outside_the_index() {
    let dir = fixture("plain");
    let mut index = CodeIndex::build(dir.path(), 2);
    let notes = write(dir.path(), "notes.txt", "class NotKotlin");
    let build_output = write(dir.path(), "build/tmp/Stub.kt", "class Stub");

    let stats = index.apply_changes(&[notes, build_output], 1);

    assert_eq!(stats, UpdateStats::default());
}

#[test]
fn refresh_picks_up_added_changed_and_deleted_files() {
    let dir = fixture("plain");
    let mut index = CodeIndex::build(dir.path(), 2);
    write(dir.path(), "src/main/kotlin/New.kt", "class AddedLater\n");
    write(
        dir.path(),
        "src/test/kotlin/org/acme/UserViewModelTest.kt",
        "package org.acme\nclass ChangedTest\n",
    );
    fs::remove_file(
        dir.path()
            .join("src/main/java/org/acme/legacy/LegacyUserService.java"),
    )
    .expect("delete file");

    let stats = index.refresh(2);

    assert_eq!(
        stats,
        UpdateStats {
            files_parsed: 2,
            files_removed: 1,
        }
    );
    assert_eq!(
        names(&index.search("AddedLater", SearchScope::Types, 1)),
        vec!["AddedLater"]
    );
    assert_eq!(
        names(&index.search("ChangedTest", SearchScope::Types, 1)),
        vec!["ChangedTest"]
    );
    assert_eq!(
        names(&index.search("LegacyUserService", SearchScope::Types, 1)),
        Vec::<&str>::new()
    );
}

#[test]
fn refresh_without_changes_parses_nothing() {
    let dir = fixture("kmp");
    let mut index = CodeIndex::build(dir.path(), 2);

    let stats = index.refresh(2);

    assert_eq!(stats, UpdateStats::default());
}

#[test]
fn stats_count_files_and_symbols() {
    let dir = fixture("kmp");

    let stats = CodeIndex::build(dir.path(), 2).stats();

    assert_eq!(stats.files, 5);
    assert_eq!(stats.symbols, 23);
    assert!(stats.heap_bytes > 0);
}

#[test]
fn file_outline_lists_declarations_in_source_order() {
    let dir = fixture("kmp");
    let index = CodeIndex::build(dir.path(), 2);

    let symbols =
        index.file_symbols("shared/src/androidMain/kotlin/com/example/shared/Platform.android.kt");

    assert_eq!(
        symbols
            .iter()
            .map(|symbol| (symbol.kind, symbol.name.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (SymbolKind::Class, "Platform"),
            (SymbolKind::Property, "name"),
            (SymbolKind::Function, "platformName"),
        ]
    );
}
