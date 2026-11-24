use super::*;

/// A `ColorMap` with a distinct color per field, so a test can tell which field a capture
/// resolved to. Channel values are the field index; the alpha marks the map as the test map.
fn test_color_map() -> ColorMap {
    let color = |tag: u8| ColorU::new(tag, tag, tag, 1);
    ColorMap {
        keyword_color: color(1),
        string_color: color(2),
        number_color: color(3),
        comment_color: color(4),
        doc_comment_color: color(5),
        type_color: color(6),
        function_declaration_color: color(7),
        function_call_color: color(8),
        field_color: color(9),
        plain_default_color: color(10),
        string_escape_color: color(11),
        boolean_color: color(12),
        punctuation_delimiter_color: color(13),
        punctuation_bracket_color: color(14),
        annotation_color: color(15),
        namespace_color: color(16),
        operator_color: color(17),
        label_color: color(18),
    }
}

fn maps_to(name: &str, tag: u8) {
    let color_map = test_color_map();
    let expected = ColorU::new(tag, tag, tag, 1);
    assert_eq!(
        convert_capture_name_to_color(name, &color_map),
        Some(expected),
        "capture {name:?} should map to field #{tag}"
    );
}

#[test]
fn kotlin_and_java_keyword_captures_map_to_keyword_color() {
    for name in [
        "keyword",
        "keyword.import",
        "keyword.function",
        "keyword.return",
        "conditional",
        "repeat",
        "exception",
        "storageclass",
        "preproc",
        "keyword.modifier",
        "keyword.operator",
        "keyword.type",
        "keyword.conditional",
        "keyword.conditional.ternary",
        "keyword.coroutine",
        "keyword.directive",
        "keyword.exception",
    ] {
        maps_to(name, 1);
    }
}

#[test]
fn kotlin_and_java_function_captures_map_to_function_colors() {
    for name in [
        "function",
        "function.method",
        "function.definition",
        "function.macro",
        "method",
    ] {
        maps_to(name, 7);
    }
    for name in [
        "function.builtin",
        "function.call",
        "function.macro.builtin",
        "function.method.builtin",
        "function.special",
        "method.call",
    ] {
        maps_to(name, 8);
    }
}

#[test]
fn type_and_constructor_captures_map_to_type_colors() {
    for name in [
        "type",
        "type.builtin",
        "type.definition",
        "type.qualifier",
        "tag",
        "tag.attribute",
        "tag.error",
    ] {
        maps_to(name, 6);
    }
    maps_to("constructor", 10);
}

#[test]
fn variable_captures_map_to_plain_default_color() {
    for name in [
        "variable",
        "variable.builtin",
        "variable.parameter",
        "parameter",
        "parameter.builtin",
    ] {
        maps_to(name, 10);
    }
}

#[test]
fn field_and_constant_captures_map_to_field_color() {
    for name in [
        "property",
        "property.definition",
        "field",
        "variable.member",
        "constant",
        "constant.macro",
    ] {
        maps_to(name, 9);
    }
}

#[test]
fn boolean_captures_map_to_boolean_color() {
    for name in ["boolean", "constant.builtin"] {
        maps_to(name, 12);
    }
}

#[test]
fn namespace_captures_map_to_namespace_color() {
    for name in ["namespace", "module", "module.builtin"] {
        maps_to(name, 16);
    }
}

#[test]
fn literal_captures_map_to_literal_colors() {
    for name in [
        "string",
        "string.regex",
        "string.regexp",
        "string.special",
        "string.special.key",
        "string.special.path",
        "string.special.regex",
        "string.special.symbol",
        "string.special.uri",
        "character",
    ] {
        maps_to(name, 2);
    }
    for name in ["string.escape", "escape", "character.special"] {
        maps_to(name, 11);
    }
    for name in ["number", "number.float", "float"] {
        maps_to(name, 3);
    }
}

#[test]
fn comment_captures_map_to_comment_colors() {
    for name in ["comment", "comment.unused"] {
        maps_to(name, 4);
    }
    for name in [
        "comment.doc",
        "comment.documentation",
        "comment.doc.__attribute__",
    ] {
        maps_to(name, 5);
    }
}

#[test]
fn operator_and_punctuation_captures_map_to_punctuation_colors() {
    maps_to("operator", 17);
    for name in ["punctuation", "punctuation.bracket"] {
        maps_to(name, 14);
    }
    maps_to("punctuation.delimiter", 13);
    maps_to("punctuation.special", 1);
    maps_to("attribute", 15);
    maps_to("label", 18);
}

#[test]
fn markdown_text_captures_keep_their_mapping() {
    maps_to("text.title", 1);
    maps_to("text.literal", 2);
    maps_to("text.uri", 7);
    maps_to("text.reference", 9);
}

#[test]
fn unknown_and_control_captures_map_to_nothing() {
    let color_map = test_color_map();
    for name in ["none", "spell", "_class", "_function", "", "at", "media"] {
        assert_eq!(
            convert_capture_name_to_color(name, &color_map),
            None,
            "capture {name:?} should not be highlighted"
        );
    }
}
