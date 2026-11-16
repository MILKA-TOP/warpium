use std::path::{Path, PathBuf};

use super::*;
use crate::lex::position_of;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn read(root: &Path, rel_path: &str) -> String {
    std::fs::read_to_string(root.join(rel_path)).expect("fixture file exists")
}

/// Where `word` starts inside the first occurrence of `context` in `text`.
fn position_in(text: &str, context: &str, word: &str) -> Position {
    let start = text.find(context).expect("context is in the file");
    let offset = start + context.find(word).expect("word is in the context");
    position_of(text, &line_starts(text), offset)
}

fn definition(index: &CodeIndex, rel_path: &str, context: &str, word: &str) -> DefinitionResult {
    let text = read(index.root(), rel_path);
    let file = SourceFile {
        rel_path,
        text: &text,
        language: Language::Kotlin,
    };
    index
        .find_definition(&file, position_in(&text, context, word))
        .expect("an identifier is at the position")
}

fn java_definition(
    index: &CodeIndex,
    rel_path: &str,
    context: &str,
    word: &str,
) -> DefinitionResult {
    let text = read(index.root(), rel_path);
    let file = SourceFile {
        rel_path,
        text: &text,
        language: Language::Java,
    };
    index
        .find_definition(&file, position_in(&text, context, word))
        .expect("an identifier is at the position")
}

/// `(path, zero-based line, counterpart)` of each definition.
fn targets(result: &DefinitionResult) -> Vec<(&str, u32, bool)> {
    result
        .definitions
        .iter()
        .map(|definition| {
            (
                definition.symbol.rel_path.as_str(),
                definition.symbol.name_pos.line,
                definition.counterpart,
            )
        })
        .collect()
}

const VIEW_MODEL: &str =
    "feature/news/src/commonMain/kotlin/com/example/feature/news/NewsViewModel.kt";
const REPOSITORY: &str = "core/data/src/commonMain/kotlin/com/example/data/NewsRepository.kt";
const RESOURCE: &str = "core/model/src/commonMain/kotlin/com/example/model/NewsResource.kt";

#[test]
fn resolves_a_type_through_an_explicit_import() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = definition(
        &index,
        VIEW_MODEL,
        "newsRepository: NewsRepository",
        "NewsRepository",
    );

    assert_eq!(targets(&result), vec![(REPOSITORY, 4, false)]);
    assert_eq!(result.definitions[0].resolution, Resolution::Import);
    assert!(!result.at_declaration);
}

#[test]
fn resolves_a_type_through_a_star_import() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = definition(&index, VIEW_MODEL, "first: NewsResource", "NewsResource");

    assert_eq!(targets(&result), vec![(RESOURCE, 2, false)]);
    assert_eq!(result.definitions[0].resolution, Resolution::StarImport);
}

#[test]
fn resolves_a_member_through_the_declared_type_of_a_constructor_property() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = definition(
        &index,
        VIEW_MODEL,
        "newsRepository.getNewsResources()",
        "getNewsResources",
    );

    assert_eq!(targets(&result), vec![(REPOSITORY, 5, false)]);
    assert_eq!(result.definitions[0].resolution, Resolution::Receiver);
}

#[test]
fn resolves_a_member_through_a_local_variable_type() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = definition(&index, VIEW_MODEL, "first.title", "title");

    assert_eq!(targets(&result), vec![(RESOURCE, 2, false)]);
    assert_eq!(result.definitions[0].symbol.container, "NewsResource");
}

#[test]
fn resolves_a_local_variable_in_the_enclosing_function() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = definition(&index, VIEW_MODEL, "items.first()", "items");

    assert_eq!(targets(&result), vec![(VIEW_MODEL, 10, false)]);
    assert_eq!(result.definitions[0].resolution, Resolution::Local);
    assert!(result.definitions[0].local);
}

#[test]
fn resolves_a_string_template_reference_to_a_class_property() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = definition(&index, VIEW_MODEL, "$greeting", "greeting");

    assert_eq!(targets(&result), vec![(VIEW_MODEL, 7, false)]);
    assert_eq!(result.definitions[0].resolution, Resolution::File);
}

#[test]
fn resolves_a_same_package_class_and_its_companion_constant() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);
    let screen = "feature/news/src/commonMain/kotlin/com/example/feature/news/NewsScreen.kt";

    let class = definition(&index, screen, "NewsViewModel.PAGE_SIZE", "NewsViewModel");
    let constant = definition(&index, screen, "NewsViewModel.PAGE_SIZE", "PAGE_SIZE");

    assert_eq!(targets(&class), vec![(VIEW_MODEL, 6, false)]);
    assert_eq!(class.definitions[0].resolution, Resolution::Package);
    assert_eq!(targets(&constant), vec![(VIEW_MODEL, 17, false)]);
}

#[test]
fn resolves_a_callable_reference_through_its_receiver_type() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = definition(&index, VIEW_MODEL, "NewsResource::summary", "summary");

    assert_eq!(targets(&result), vec![(RESOURCE, 3, false)]);
    assert_eq!(result.definitions[0].resolution, Resolution::Receiver);
}

#[test]
fn resolves_an_import_directive_to_the_imported_declaration() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = definition(
        &index,
        VIEW_MODEL,
        "import com.example.data.NewsRepository",
        "NewsRepository",
    );

    assert_eq!(targets(&result), vec![(REPOSITORY, 4, false)]);
}

#[test]
fn common_code_goes_to_the_expect_declaration_with_actuals_as_counterparts() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = definition(&index, VIEW_MODEL, "platformName()", "platformName");

    assert_eq!(
        targets(&result),
        vec![
            (
                "core/data/src/commonMain/kotlin/com/example/data/Platform.kt",
                2,
                false
            ),
            (
                "core/data/src/androidMain/kotlin/com/example/data/Platform.android.kt",
                2,
                true
            ),
            (
                "core/data/src/iosMain/kotlin/com/example/data/Platform.ios.kt",
                2,
                true
            ),
        ]
    );
}

#[test]
fn platform_code_goes_to_the_actual_of_its_own_source_set() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);
    let android = "core/data/src/androidMain/kotlin/com/example/data/Platform.android.kt";

    let result = definition(&index, android, "+ platformName()", "platformName");

    assert_eq!(
        targets(&result),
        vec![
            (android, 2, false),
            (
                "core/data/src/commonMain/kotlin/com/example/data/Platform.kt",
                2,
                true
            ),
            (
                "core/data/src/iosMain/kotlin/com/example/data/Platform.ios.kt",
                2,
                true
            ),
        ]
    );
}

#[test]
fn a_declaration_name_resolves_to_itself() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = definition(
        &index,
        REPOSITORY,
        "fun getNewsResources",
        "getNewsResources",
    );

    assert!(result.at_declaration);
    assert_eq!(targets(&result), vec![(REPOSITORY, 5, false)]);
}

#[test]
fn a_constructed_receiver_resolves_to_the_override_in_its_class() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);
    let test = "feature/news/src/commonTest/kotlin/com/example/feature/news/NewsViewModelTest.kt";

    let result = definition(
        &index,
        test,
        "repository.getNewsResources()",
        "getNewsResources",
    );

    assert_eq!(
        targets(&result),
        vec![(
            "core/data-test/src/commonMain/kotlin/com/example/data/test/FakeNewsRepository.kt",
            6,
            false
        )]
    );
    assert_eq!(result.definitions[0].resolution, Resolution::Receiver);
}

#[test]
fn an_unknown_receiver_falls_back_to_members_with_the_name() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = definition(&index, VIEW_MODEL, "it.id", "id");

    assert_eq!(targets(&result), vec![(RESOURCE, 2, false)]);
    assert_eq!(result.definitions[0].resolution, Resolution::Name);
}

#[test]
fn resolves_against_unsaved_text() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);
    let text =
        "package com.example.feature.news\n\nfun draft() {\n    val fresh = 1\n    fresh\n}\n";
    let file = SourceFile {
        rel_path: VIEW_MODEL,
        text,
        language: Language::Kotlin,
    };

    let result = index
        .find_definition(&file, Position { line: 4, column: 6 })
        .expect("an identifier is at the position");

    assert_eq!(targets(&result), vec![(VIEW_MODEL, 3, false)]);
}

#[test]
fn a_cursor_right_after_a_name_still_finds_it() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);
    let text = read(index.root(), VIEW_MODEL);
    let file = SourceFile {
        rel_path: VIEW_MODEL,
        text: &text,
        language: Language::Kotlin,
    };
    let mut position = position_in(&text, "items.first()", "items");
    position.column += "items".len() as u32;

    let result = index.find_definition(&file, position);

    assert_eq!(result.map(|result| result.name), Some("items".to_owned()));
}

#[test]
fn whitespace_has_no_definition() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);
    let file = SourceFile {
        rel_path: VIEW_MODEL,
        text: "package a\n\n\n",
        language: Language::Kotlin,
    };

    assert_eq!(
        index.find_definition(&file, Position { line: 2, column: 0 }),
        None
    );
}

const SCREEN: &str = "src/main/kotlin/org/acme/UserScreen.kt";
const LEGACY: &str = "src/main/java/org/acme/legacy/LegacyUserService.java";

#[test]
fn resolves_an_alias_import_to_a_java_class() {
    let index = CodeIndex::build(fixture("nav-plain"), 1);

    let result = definition(&index, SCREEN, "Legacy()", "Legacy");

    assert_eq!(targets(&result), vec![(LEGACY, 4, false)]);
}

#[test]
fn resolves_a_java_method_through_a_constructed_property_type() {
    let index = CodeIndex::build(fixture("nav-plain"), 1);

    let result = definition(&index, SCREEN, "service.find", "find");

    assert_eq!(targets(&result), vec![(LEGACY, 8, false)]);
    assert_eq!(result.definitions[0].resolution, Resolution::Receiver);
}

#[test]
fn resolves_a_static_java_field_through_an_aliased_class() {
    let index = CodeIndex::build(fixture("nav-plain"), 1);

    let result = definition(&index, SCREEN, "Legacy.MAX_USERS", "MAX_USERS");

    assert_eq!(targets(&result), vec![(LEGACY, 5, false)]);
}

#[test]
fn resolves_an_extension_function_in_the_same_package() {
    let index = CodeIndex::build(fixture("nav-plain"), 1);

    let result = definition(&index, SCREEN, "user.initials()", "initials");

    assert_eq!(
        targets(&result),
        vec![("src/main/kotlin/org/acme/User.kt", 4, false)]
    );
    assert_eq!(result.definitions[0].resolution, Resolution::Package);
}

#[test]
fn java_resolves_a_kotlin_class_through_an_import() {
    let index = CodeIndex::build(fixture("nav-plain"), 1);

    let result = java_definition(&index, LEGACY, "public User find", "User");

    assert_eq!(
        targets(&result),
        vec![("src/main/kotlin/org/acme/User.kt", 2, false)]
    );
}

#[test]
fn java_resolves_a_method_through_a_field_of_a_nested_class_type() {
    let index = CodeIndex::build(fixture("nav-plain"), 1);

    let result = java_definition(&index, LEGACY, "cache.lookup", "lookup");

    assert_eq!(targets(&result), vec![(LEGACY, 14, false)]);
    assert_eq!(
        result.definitions[0].symbol.container,
        "LegacyUserService.Cache"
    );
}

#[test]
fn java_resolves_a_local_variable() {
    let index = CodeIndex::build(fixture("nav-plain"), 1);

    let result = java_definition(&index, LEGACY, "return user", "user");

    assert_eq!(targets(&result), vec![(LEGACY, 9, false)]);
    assert!(result.definitions[0].local);
}

/// Writes `files` (path, contents) into a temporary project.
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
fn java_method_calls_skip_same_named_local_variables() {
    let source = "package p;\n\nclass Box {\n    int size() { return 0; }\n\n    int twice() {\n        int size = size();\n        return size() + size;\n    }\n}\n";
    let dir = project(&[("src/main/java/p/Box.java", source)]);
    let index = CodeIndex::build(dir.path(), 1);

    let result = java_definition(&index, "src/main/java/p/Box.java", "return size()", "size");

    assert_eq!(
        targets(&result),
        vec![("src/main/java/p/Box.java", 3, false)]
    );
    assert!(!result.definitions[0].local);
}

#[test]
fn kotlin_function_typed_parameters_are_callable_locals() {
    let source = "package p\n\nfun button(onClick: () -> Unit) {\n    onClick()\n}\n";
    let dir = project(&[("src/main/kotlin/p/Button.kt", source)]);
    let index = CodeIndex::build(dir.path(), 1);

    let result = definition(
        &index,
        "src/main/kotlin/p/Button.kt",
        "    onClick()",
        "onClick",
    );

    assert_eq!(
        targets(&result),
        vec![("src/main/kotlin/p/Button.kt", 2, false)]
    );
    assert!(result.definitions[0].local);
}

#[test]
fn plain_constructor_parameters_are_not_visible_in_member_functions() {
    let source = "package p\n\nclass Foo(name: String) {\n    val name = name.trim()\n\n    fun f() = name\n}\n";
    let dir = project(&[("src/main/kotlin/p/Foo.kt", source)]);
    let index = CodeIndex::build(dir.path(), 1);

    let in_function = definition(&index, "src/main/kotlin/p/Foo.kt", "fun f() = name", "name");
    let in_initializer = definition(&index, "src/main/kotlin/p/Foo.kt", "= name.trim()", "name");

    assert_eq!(
        targets(&in_function),
        vec![("src/main/kotlin/p/Foo.kt", 3, false)]
    );
    assert!(!in_function.definitions[0].local);
    assert_eq!(
        targets(&in_initializer),
        vec![("src/main/kotlin/p/Foo.kt", 2, false)]
    );
    assert!(in_initializer.definitions[0].local);
}

const MULTIBYTE_SOURCE: &str = "package p\n\n// Привет 😀\nclass Имя {\n    fun go() = \"ц\"\n}\n";

fn definition_in_multibyte_file(position: Position) -> Option<DefinitionResult> {
    let index = CodeIndex::new("/nonexistent");
    let file = SourceFile {
        rel_path: "src/main/kotlin/p/Name.kt",
        text: MULTIBYTE_SOURCE,
        language: Language::Kotlin,
    };
    index.find_definition(&file, position)
}

#[test]
fn a_position_past_the_last_line_has_no_definition() {
    let result = definition_in_multibyte_file(Position {
        line: 99,
        column: 0,
    });

    assert!(result.is_none());
}

#[test]
fn a_column_past_the_end_of_a_multibyte_comment_has_no_definition() {
    let result = definition_in_multibyte_file(Position {
        line: 2,
        column: 500,
    });

    assert!(result.is_none());
}

#[test]
fn a_multibyte_declaration_name_resolves_to_itself() {
    let result = definition_in_multibyte_file(Position { line: 3, column: 7 })
        .expect("an identifier is at the position");

    assert_eq!(result.name, "Имя");
    assert!(result.at_declaration);
}

#[test]
fn usages_past_the_end_of_the_file_are_none() {
    let index = CodeIndex::new("/nonexistent");
    let file = SourceFile {
        rel_path: "src/main/kotlin/p/Name.kt",
        text: MULTIBYTE_SOURCE,
        language: Language::Kotlin,
    };

    let result = index.find_usages(&file, Position { line: 6, column: 3 }, 10, 1);

    assert!(result.is_none());
}
