use super::*;

fn hex(color: ColorU) -> String {
    format!("{:02X}{:02X}{:02X}", color.r, color.g, color.b)
}

#[test]
fn warpium_dark_palette_matches_jetbrains_dark_scheme() {
    let palette = palette(true);
    assert_eq!(hex(palette.keyword), "CF8E6D");
    assert_eq!(hex(palette.string), "6AAB73");
    assert_eq!(hex(palette.number), "2AACB8");
    assert_eq!(hex(palette.comment), "7A7E85");
    assert_eq!(hex(palette.doc_comment), "5F826B");
    assert_eq!(hex(palette.function_declaration), "56A8F5");
    assert_eq!(hex(palette.function_call), "BCBEC4");
    assert_eq!(hex(palette.field), "C77DBB");
    assert_eq!(hex(palette.plain_default), "BCBEC4");
    assert_eq!(hex(palette.string_escape), "CF8E6D");
    assert_eq!(hex(palette.boolean), "CF8E6D");
    assert_eq!(hex(palette.punctuation_delimiter), "CF8E6D");
    assert_eq!(hex(palette.punctuation_bracket), "BCBEC4");
    assert_eq!(hex(palette.annotation), "BCBEC4");
    assert_eq!(hex(palette.namespace), "BCBEC4");
    assert_eq!(hex(palette.operator), "BCBEC4");
    assert_eq!(hex(palette.label), "BCBEC4");
}

#[test]
fn warpium_light_palette_matches_intellij_light_scheme() {
    let palette = palette(false);
    assert_eq!(hex(palette.keyword), "0033B3");
    assert_eq!(hex(palette.string), "067D17");
    assert_eq!(hex(palette.number), "1750EB");
    assert_eq!(hex(palette.comment), "8C8C8C");
    assert_eq!(hex(palette.doc_comment), "8C8C8C");
    assert_eq!(hex(palette.function_declaration), "00627A");
    assert_eq!(hex(palette.function_call), "080808");
    assert_eq!(hex(palette.field), "871094");
    assert_eq!(hex(palette.plain_default), "080808");
    assert_eq!(hex(palette.string_escape), "0037A6");
    assert_eq!(hex(palette.boolean), "0033B3");
    assert_eq!(hex(palette.punctuation_delimiter), "0033B3");
    assert_eq!(hex(palette.punctuation_bracket), "080808");
    assert_eq!(hex(palette.annotation), "080808");
    assert_eq!(hex(palette.namespace), "080808");
    assert_eq!(hex(palette.operator), "080808");
    assert_eq!(hex(palette.label), "080808");
}

#[test]
fn warpium_color_map_uses_the_scheme_matching_the_theme() {
    let dark = syntax_color_map(ColorScheme::LightOnDark);
    assert_eq!(hex(dark.keyword_color), "CF8E6D");
    assert_eq!(hex(dark.string_color), "6AAB73");
    assert_eq!(hex(dark.number_color), "2AACB8");
    assert_eq!(hex(dark.comment_color), "7A7E85");
    assert_eq!(hex(dark.doc_comment_color), "5F826B");
    assert_eq!(hex(dark.function_declaration_color), "56A8F5");
    assert_eq!(hex(dark.field_color), "C77DBB");
    assert_eq!(hex(dark.type_color), "BCBEC4");
    assert_eq!(hex(dark.plain_default_color), "BCBEC4");
    assert_eq!(hex(dark.string_escape_color), "CF8E6D");
    assert_eq!(hex(dark.boolean_color), "CF8E6D");
    assert_eq!(hex(dark.punctuation_delimiter_color), "CF8E6D");
    assert_eq!(hex(dark.punctuation_bracket_color), "BCBEC4");
    assert_eq!(hex(dark.annotation_color), "BCBEC4");
    assert_eq!(hex(dark.namespace_color), "BCBEC4");
    assert_eq!(hex(dark.operator_color), "BCBEC4");
    assert_eq!(hex(dark.label_color), "BCBEC4");

    let light = syntax_color_map(ColorScheme::DarkOnLight);
    assert_eq!(hex(light.keyword_color), "0033B3");
    assert_eq!(hex(light.string_color), "067D17");
    assert_eq!(hex(light.number_color), "1750EB");
    assert_eq!(hex(light.comment_color), "8C8C8C");
    assert_eq!(hex(light.doc_comment_color), "8C8C8C");
    assert_eq!(hex(light.function_declaration_color), "00627A");
    assert_eq!(hex(light.field_color), "871094");
    assert_eq!(hex(light.type_color), "080808");
    assert_eq!(hex(light.plain_default_color), "080808");
    assert_eq!(hex(light.string_escape_color), "0037A6");
    assert_eq!(hex(light.boolean_color), "0033B3");
    assert_eq!(hex(light.punctuation_delimiter_color), "0033B3");
    assert_eq!(hex(light.punctuation_bracket_color), "080808");
    assert_eq!(hex(light.annotation_color), "080808");
    assert_eq!(hex(light.namespace_color), "080808");
    assert_eq!(hex(light.operator_color), "080808");
    assert_eq!(hex(light.label_color), "080808");
}
