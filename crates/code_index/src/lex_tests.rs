use super::*;
use crate::Position;

fn identifiers(source: &str, language: Language) -> Vec<String> {
    let mut found = Vec::new();
    for_each_identifier(source, language, |name, _| found.push(name.to_owned()));
    found
}

#[test]
fn skips_comments_and_strings() {
    let source = "val a = b // c\n/* d */ e(\"f g\", 'h')";

    assert_eq!(identifiers(source, Language::Kotlin), vec!["val", "a", "b", "e"]);
}

#[test]
fn reports_kotlin_string_template_references() {
    let source = r#"println("hi $name and ${user.id + "x$y"} done")"#;

    assert_eq!(
        identifiers(source, Language::Kotlin),
        vec!["println", "name", "user", "id", "y"]
    );
}

#[test]
fn kotlin_block_comments_nest() {
    let source = "/* a /* b */ c */ d";

    assert_eq!(identifiers(source, Language::Kotlin), vec!["d"]);
}

#[test]
fn java_block_comments_do_not_nest() {
    let source = "/* a /* b */ c */ d";

    assert_eq!(identifiers(source, Language::Java), vec!["c", "d"]);
}

#[test]
fn raw_strings_end_at_the_last_of_trailing_quotes() {
    let source = "x = \"\"\"a \"quoted\" b\"\"\"\" + z";

    assert_eq!(identifiers(source, Language::Kotlin), vec!["x", "z"]);
}

#[test]
fn backtick_identifiers_are_reported_without_backticks() {
    let source = "fun `my test`() = `in`";

    assert_eq!(
        identifiers(source, Language::Kotlin),
        vec!["fun", "my test", "in"]
    );
}

#[test]
fn java_identifiers_may_contain_dollar_signs() {
    assert_eq!(
        identifiers("Outer$Inner x = \"$no\";", Language::Java),
        vec!["Outer$Inner", "x"]
    );
}

#[test]
fn escaped_quotes_stay_inside_strings() {
    let source = r#"a("\"b", '\'') c"#;

    assert_eq!(identifiers(source, Language::Java), vec!["a", "c"]);
}

#[test]
fn number_suffixes_are_not_identifiers() {
    assert_eq!(identifiers("x = 0xFFL + 1e10f", Language::Kotlin), vec!["x"]);
}

#[test]
fn converts_offsets_to_character_positions_and_back() {
    let source = "ab\nцd x\n";
    let starts = line_starts(source);
    let x = source.find('x').expect("x is in the source");

    let position = position_of(source, &starts, x);

    assert_eq!(position, Position { line: 1, column: 3 });
    assert_eq!(offset_of(source, &starts, position), Some(x));
}

#[test]
fn offset_of_clamps_columns_past_the_line_end() {
    let source = "ab\ncd";

    assert_eq!(
        offset_of(source, &line_starts(source), Position { line: 0, column: 9 }),
        Some(2)
    );
}
