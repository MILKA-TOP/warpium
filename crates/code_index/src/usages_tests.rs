use std::path::{Path, PathBuf};

use super::*;
use crate::lex::{line_starts, position_of};
use crate::model::Language;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn usages_at(
    index: &CodeIndex,
    rel_path: &str,
    language: Language,
    context: &str,
    word: &str,
) -> UsagesResult {
    let text = std::fs::read_to_string(index.root().join(rel_path)).expect("fixture file exists");
    let start = text.find(context).expect("context is in the file");
    let offset = start + context.find(word).expect("word is in the context");
    let file = SourceFile {
        rel_path,
        text: &text,
        language,
    };
    index
        .find_usages(
            &file,
            position_of(&text, &line_starts(&text), offset),
            100,
            2,
        )
        .expect("an identifier is at the position")
}

/// `(path, zero-based line, exact)` of each usage.
fn found(result: &UsagesResult) -> Vec<(&str, u32, bool)> {
    result
        .usages
        .iter()
        .map(|usage| (usage.rel_path.as_str(), usage.position.line, usage.exact))
        .collect()
}

/// `(path, zero-based line, via)` of each usage.
fn found_via(result: &UsagesResult) -> Vec<(&str, u32, bool)> {
    result
        .usages
        .iter()
        .map(|usage| (usage.rel_path.as_str(), usage.position.line, usage.via))
        .collect()
}

const VIEW_MODEL: &str =
    "feature/news/src/commonMain/kotlin/com/example/feature/news/NewsViewModel.kt";
const REPOSITORY: &str = "core/data/src/commonMain/kotlin/com/example/data/NewsRepository.kt";
const RESOURCE: &str = "core/model/src/commonMain/kotlin/com/example/model/NewsResource.kt";
const FAKE: &str =
    "core/data-test/src/commonMain/kotlin/com/example/data/test/FakeNewsRepository.kt";

const VIEW_MODEL_TEST: &str =
    "feature/news/src/commonTest/kotlin/com/example/feature/news/NewsViewModelTest.kt";

#[test]
fn interface_method_usages_leave_out_override_declarations_comments_and_strings() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = usages_at(
        &index,
        REPOSITORY,
        Language::Kotlin,
        "fun getNewsResources",
        "getNewsResources",
    );

    assert_eq!(
        found(&result),
        vec![(VIEW_MODEL, 10, true), (VIEW_MODEL_TEST, 7, true)]
    );
    assert!(result.complete);
}

#[test]
fn interface_method_usages_list_calls_on_implementations_last_as_via() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = usages_at(
        &index,
        REPOSITORY,
        Language::Kotlin,
        "fun getNewsResources",
        "getNewsResources",
    );

    assert_eq!(
        found_via(&result),
        vec![(VIEW_MODEL, 10, false), (VIEW_MODEL_TEST, 7, true)]
    );
    assert_eq!(
        result
            .via
            .iter()
            .map(|symbol| symbol.container.as_str())
            .collect::<Vec<_>>(),
        vec!["OfflineFirstNewsRepository", "FakeNewsRepository"]
    );
}

#[test]
fn member_usages_through_subtypes_resolve_to_the_override_they_call() {
    let index = CodeIndex::build(fixture("hierarchy"), 1);

    let result = usages_at(
        &index,
        "shapes/src/main/kotlin/com/acme/shapes/Shape.kt",
        Language::Kotlin,
        "fun area",
        "area",
    );

    let render = "app/src/main/kotlin/com/acme/app/Render.kt";
    assert_eq!(
        found_via(&result),
        vec![(render, 6, false), (render, 8, true), (render, 10, true)]
    );
}

#[test]
fn usages_of_an_override_do_not_include_its_siblings() {
    let index = CodeIndex::build(fixture("hierarchy"), 1);

    let result = usages_at(
        &index,
        "shapes/src/main/kotlin/com/acme/shapes/Circle.kt",
        Language::Kotlin,
        "fun area",
        "area",
    );

    assert_eq!(
        found_via(&result),
        vec![("app/src/main/kotlin/com/acme/app/Render.kt", 8, false)]
    );
    assert!(result.via.is_empty());
}

#[test]
fn class_usages_cover_imports_and_type_references_with_test_fixtures_last() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = usages_at(
        &index,
        RESOURCE,
        Language::Kotlin,
        "class NewsResource",
        "NewsResource",
    );

    assert_eq!(
        found(&result),
        vec![
            (REPOSITORY, 2, true),
            (REPOSITORY, 5, true),
            (REPOSITORY, 9, true),
            (REPOSITORY, 13, true),
            (VIEW_MODEL, 9, true),
            (VIEW_MODEL, 11, true),
            (VIEW_MODEL, 21, true),
            (VIEW_MODEL, 21, true),
            (FAKE, 3, true),
            (FAKE, 6, true),
        ]
    );
}

#[test]
fn local_variable_usages_stay_in_the_file() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = usages_at(&index, VIEW_MODEL, Language::Kotlin, "val items", "items");

    assert_eq!(
        found(&result),
        vec![(VIEW_MODEL, 11, true), (VIEW_MODEL, 13, true)]
    );
    assert_eq!(result.files_scanned, 1);
}

#[test]
fn expect_function_usages_include_calls_resolved_to_actuals() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = usages_at(
        &index,
        VIEW_MODEL,
        Language::Kotlin,
        "platformName()",
        "platformName",
    );

    assert_eq!(
        found_via(&result),
        vec![
            (VIEW_MODEL, 3, false),
            (VIEW_MODEL, 13, false),
            (
                "core/data/src/androidMain/kotlin/com/example/data/Platform.android.kt",
                4,
                true
            ),
        ]
    );
    assert_eq!(result.targets.len(), 1);
    assert_eq!(result.via.len(), 2);
}

#[test]
fn usages_report_the_line_text() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = usages_at(&index, VIEW_MODEL, Language::Kotlin, "val items", "items");

    assert_eq!(
        result.usages[0].line_text,
        "val first: NewsResource = items.first()"
    );
}

#[test]
fn usages_stop_at_the_limit() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);
    let text = std::fs::read_to_string(index.root().join(RESOURCE)).expect("fixture file exists");
    let file = SourceFile {
        rel_path: RESOURCE,
        text: &text,
        language: Language::Kotlin,
    };

    let result = index
        .find_usages(
            &file,
            Position {
                line: 2,
                column: 11,
            },
            3,
            1,
        )
        .expect("an identifier is at the position");

    assert_eq!(result.usages.len(), 3);
    assert!(!result.complete);
}

const LEGACY: &str = "src/main/java/org/acme/legacy/LegacyUserService.java";

#[test]
fn kotlin_class_usages_include_java_code() {
    let index = CodeIndex::build(fixture("nav-plain"), 1);

    let result = usages_at(
        &index,
        "src/main/kotlin/org/acme/User.kt",
        Language::Kotlin,
        "class User",
        "User",
    );

    assert_eq!(
        found(&result),
        vec![
            (LEGACY, 2, true),
            (LEGACY, 8, true),
            (LEGACY, 9, true),
            (LEGACY, 14, true),
            ("src/main/kotlin/org/acme/User.kt", 4, true),
        ]
    );
}

#[test]
fn java_method_usages_include_kotlin_calls() {
    let index = CodeIndex::build(fixture("nav-plain"), 1);

    let result = usages_at(&index, LEGACY, Language::Java, "User find", "find");

    assert_eq!(
        found(&result),
        vec![("src/main/kotlin/org/acme/UserScreen.kt", 8, true)]
    );
}

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp dir");
    for (rel_path, contents) in files {
        let path = dir.path().join(rel_path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        std::fs::write(path, contents).expect("write file");
    }
    dir
}

#[test]
fn star_imports_of_a_package_see_its_nested_types() {
    let dir = project(&[
        (
            "src/main/kotlin/p/Outer.kt",
            "package p\n\nclass Outer {\n    class Inner\n}\n",
        ),
        (
            "src/main/kotlin/q/User.kt",
            "package q\n\nimport p.*\n\nval inner = Outer.Inner()\n",
        ),
    ]);
    let index = CodeIndex::build(dir.path(), 1);

    let result = usages_at(
        &index,
        "src/main/kotlin/p/Outer.kt",
        Language::Kotlin,
        "class Inner",
        "Inner",
    );

    assert_eq!(found(&result), vec![("src/main/kotlin/q/User.kt", 4, true)]);
}

#[test]
fn parameters_and_named_arguments_with_the_name_are_not_usages() {
    let dir = project(&[
        ("src/main/kotlin/p/Load.kt", "package p\n\nfun load() {}\n"),
        (
            "src/main/kotlin/q/Run.kt",
            "package q\n\nimport p.load\n\nfun run(load: Boolean) {\n    if (load) {}\n    configure(load = true)\n    p.load()\n}\n\nfun configure(load: Boolean) {}\n\nfun go() = load()\n",
        ),
    ]);
    let index = CodeIndex::build(dir.path(), 1);

    let result = usages_at(
        &index,
        "src/main/kotlin/p/Load.kt",
        Language::Kotlin,
        "fun load",
        "load",
    );

    assert_eq!(
        found(&result),
        vec![
            ("src/main/kotlin/q/Run.kt", 2, true),
            ("src/main/kotlin/q/Run.kt", 7, true),
            ("src/main/kotlin/q/Run.kt", 12, true),
        ]
    );
}

#[test]
fn calls_on_a_known_subtype_count_as_via_in_files_that_also_name_the_interface() {
    let dir = project(&[
        ("Repo.kt", "interface Repo {\n    fun load(x: Int)\n}\n"),
        (
            "DbRepo.kt",
            "class DbRepo : Repo {\n    override fun load(x: Int) {}\n}\n",
        ),
        (
            "Main.kt",
            "fun main() {\n    DbRepo().load(2)\n    val db = DbRepo()\n    db.load(3)\n    val repo: Repo = DbRepo()\n    repo.load(4)\n}\n",
        ),
    ]);
    let index = CodeIndex::build(dir.path(), 1);

    let result = usages_at(&index, "Repo.kt", Language::Kotlin, "fun load", "load");

    assert_eq!(
        found_via(&result),
        vec![
            ("Main.kt", 5, false),
            ("Main.kt", 1, true),
            ("Main.kt", 3, true)
        ]
    );
}

#[test]
fn calls_whose_receiver_type_is_not_written_count_as_usages_of_the_interface_member() {
    let dir = project(&[
        ("Repo.kt", "interface Repo {\n    fun load(x: Int)\n}\n"),
        (
            "DbRepo.kt",
            "class DbRepo : Repo {\n    override fun load(x: Int) {}\n}\n\nfun makeRepo() = DbRepo()\n",
        ),
        (
            "Main.kt",
            "fun main(repo: Repo) {\n    repo.load(1)\n    makeRepo().load(2)\n}\n",
        ),
    ]);
    let index = CodeIndex::build(dir.path(), 1);

    let result = usages_at(&index, "Repo.kt", Language::Kotlin, "fun load", "load");

    assert_eq!(
        found_via(&result),
        vec![("Main.kt", 1, false), ("Main.kt", 2, false)]
    );
}
