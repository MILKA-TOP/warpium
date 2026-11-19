use std::path::{Path, PathBuf};

use super::*;
use crate::lex::{line_starts, position_of};
use crate::model::Language;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn implementations_at(
    index: &CodeIndex,
    rel_path: &str,
    context: &str,
    word: &str,
) -> ImplementationsResult {
    let text = std::fs::read_to_string(index.root().join(rel_path)).expect("fixture file exists");
    let start = text.find(context).expect("context is in the file");
    let offset = start + context.find(word).expect("word is in the context");
    let language = Language::from_path(Path::new(rel_path)).expect("a source file");
    let file = SourceFile {
        rel_path,
        text: &text,
        language,
    };
    index
        .find_implementations(&file, position_of(&text, &line_starts(&text), offset))
        .expect("an identifier is at the position")
}

/// `(container.name, zero-based line)` of each implementation.
fn found(result: &ImplementationsResult) -> Vec<(String, u32)> {
    result
        .implementations
        .iter()
        .map(|symbol| {
            (
                join_container(&symbol.container, &symbol.name),
                symbol.name_pos.line,
            )
        })
        .collect()
}

const SHAPE: &str = "shapes/src/main/kotlin/com/acme/shapes/Shape.kt";
const OTHER_SHAPE: &str = "shapes/src/main/kotlin/com/acme/other/Shape.kt";

#[test]
fn interface_implementations_include_indirect_and_java_subtypes() {
    let index = CodeIndex::build(fixture("hierarchy"), 1);

    let result = implementations_at(&index, SHAPE, "interface Shape", "Shape");

    assert_eq!(
        found(&result),
        vec![
            ("JavaTriangle".to_owned(), 4),
            ("Circle".to_owned(), 2),
            ("Polygon".to_owned(), 7),
            ("Square".to_owned(), 4),
        ]
    );
    assert!(result.complete);
}

#[test]
fn a_same_named_type_in_another_package_is_not_a_supertype() {
    let index = CodeIndex::build(fixture("hierarchy"), 1);

    let result = implementations_at(&index, OTHER_SHAPE, "interface Shape", "Shape");

    assert_eq!(found(&result), vec![("Blob".to_owned(), 6)]);
}

#[test]
fn interface_method_implementations_cover_kotlin_and_java_overrides() {
    let index = CodeIndex::build(fixture("hierarchy"), 1);

    let result = implementations_at(&index, SHAPE, "fun area", "area");

    assert_eq!(
        found(&result),
        vec![
            ("JavaTriangle.area".to_owned(), 6),
            ("Circle.area".to_owned(), 3),
            ("Square.area".to_owned(), 5),
        ]
    );
}

#[test]
fn property_implementations_include_constructor_properties() {
    let index = CodeIndex::build(fixture("hierarchy"), 1);

    let result = implementations_at(&index, SHAPE, "val name", "name");

    assert_eq!(
        found(&result),
        vec![("Circle.name".to_owned(), 4), ("Square.name".to_owned(), 4)]
    );
}

#[test]
fn a_reference_through_a_subtype_finds_overrides_below_it() {
    let index = CodeIndex::build(fixture("hierarchy"), 1);

    let result = implementations_at(&index, SHAPE, "abstract fun sides", "sides");

    assert_eq!(
        found(&result),
        vec![
            ("JavaTriangle.sides".to_owned(), 8),
            ("Square.sides".to_owned(), 6)
        ]
    );
}

#[test]
fn expect_implementations_are_its_actuals() {
    let index = CodeIndex::build(fixture("nav-kmp"), 1);

    let result = implementations_at(
        &index,
        "core/data/src/commonMain/kotlin/com/example/data/Platform.kt",
        "expect fun platformName",
        "platformName",
    );

    assert_eq!(
        result
            .implementations
            .iter()
            .map(|symbol| symbol.rel_path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "core/data/src/androidMain/kotlin/com/example/data/Platform.android.kt",
            "core/data/src/iosMain/kotlin/com/example/data/Platform.ios.kt",
        ]
    );
}

#[test]
fn a_class_without_subtypes_has_no_implementations() {
    let index = CodeIndex::build(fixture("hierarchy"), 1);

    let result = implementations_at(
        &index,
        "shapes/src/main/kotlin/com/acme/shapes/Circle.kt",
        "class Circle",
        "Circle",
    );

    assert_eq!(found(&result), Vec::<(String, u32)>::new());
    assert!(result.complete);
}

#[test]
fn supertypes_survive_a_snapshot_round_trip() {
    let index = CodeIndex::build(fixture("hierarchy"), 1);
    let dir = tempfile::tempdir().expect("temp dir");
    let snapshot = dir.path().join("index.bin");
    index.save(&snapshot).expect("snapshot saves");

    let loaded = CodeIndex::load(&snapshot, index.root()).expect("snapshot loads");

    assert_eq!(
        found(&implementations_at(&loaded, SHAPE, "fun area", "area")),
        found(&implementations_at(&index, SHAPE, "fun area", "area")),
    );
}
