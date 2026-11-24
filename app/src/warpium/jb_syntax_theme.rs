//! JetBrains IDE / Android Studio default editor colors for code syntax highlighting
//! (roadmap item 16).
//!
//! The values are extracted from JetBrains' intellij-community repository (Apache-2.0):
//!
//! - Dark: the new-UI "Dark" color scheme (`expUI_darkScheme.xml`), the default of current
//!   IntelliJ IDEA / Android Studio in dark mode. Editor background #1E1F22, default text
//!   #BCBEC4. Classic Darcula (`DefaultColorSchemesManager.xml`) for comparison: keyword
//!   #CC7832, string #6A8759, number #6897BB, comment #808080, doc comment #629755,
//!   function declaration #FFC66D, field #9876AA.
//! - Light: the "IntelliJ Light" scheme (`expUI_lightScheme.xml`). Background #FFFFFF,
//!   default text #080808.
//!
//! Roles the schemes leave at the default text color (class references, constructors,
//! parameters, locals, operators, brackets, annotations, packages, labels) still get their
//! own palette entries so custom themes can deviate.

use pathfinder_color::ColorU;
use syntax_tree::ColorMap;
use warp_core::ui::theme::ColorScheme;

// Dark palette (new-UI "Dark"), as 0xRRGGBBAA values.
pub(crate) const DARK_KEYWORD: u32 = 0xCF8E6DFF;
pub(crate) const DARK_STRING: u32 = 0x6AAB73FF;
pub(crate) const DARK_NUMBER: u32 = 0x2AACB8FF;
pub(crate) const DARK_COMMENT: u32 = 0x7A7E85FF;
pub(crate) const DARK_DOC_COMMENT: u32 = 0x5F826BFF;
pub(crate) const DARK_FUNCTION_DECLARATION: u32 = 0x56A8F5FF;
pub(crate) const DARK_FIELD: u32 = 0xC77DBBFF;
pub(crate) const DARK_DEFAULT_TEXT: u32 = 0xBCBEC4FF;
pub(crate) const DARK_STRING_ESCAPE: u32 = 0xCF8E6DFF;

// Light palette ("IntelliJ Light"), as 0xRRGGBBAA values.
pub(crate) const LIGHT_KEYWORD: u32 = 0x0033B3FF;
pub(crate) const LIGHT_STRING: u32 = 0x067D17FF;
pub(crate) const LIGHT_NUMBER: u32 = 0x1750EBFF;
pub(crate) const LIGHT_COMMENT: u32 = 0x8C8C8CFF;
pub(crate) const LIGHT_DOC_COMMENT: u32 = 0x8C8C8CFF;
pub(crate) const LIGHT_FUNCTION_DECLARATION: u32 = 0x00627AFF;
pub(crate) const LIGHT_FIELD: u32 = 0x871094FF;
pub(crate) const LIGHT_DEFAULT_TEXT: u32 = 0x080808FF;
pub(crate) const LIGHT_STRING_ESCAPE: u32 = 0x0037A6FF;

/// One JetBrains palette: a color for every role the default schemes distinguish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct JbSyntaxPalette {
    pub keyword: ColorU,
    pub string: ColorU,
    pub number: ColorU,
    pub comment: ColorU,
    pub doc_comment: ColorU,
    pub function_declaration: ColorU,
    pub function_call: ColorU,
    pub field: ColorU,
    pub plain_default: ColorU,
    pub string_escape: ColorU,
    pub boolean: ColorU,
    pub punctuation_delimiter: ColorU,
    pub punctuation_bracket: ColorU,
    pub annotation: ColorU,
    pub namespace: ColorU,
    pub operator: ColorU,
    pub label: ColorU,
}

/// The JetBrains palette for the given color scheme.
pub(crate) fn palette(is_dark: bool) -> JbSyntaxPalette {
    if is_dark {
        JbSyntaxPalette {
            keyword: ColorU::from_u32(DARK_KEYWORD),
            string: ColorU::from_u32(DARK_STRING),
            number: ColorU::from_u32(DARK_NUMBER),
            comment: ColorU::from_u32(DARK_COMMENT),
            doc_comment: ColorU::from_u32(DARK_DOC_COMMENT),
            function_declaration: ColorU::from_u32(DARK_FUNCTION_DECLARATION),
            // JetBrains leaves calls at the default text color; the editor's `function`
            // capture takes the declaration color because the query cannot split the two.
            function_call: ColorU::from_u32(DARK_DEFAULT_TEXT),
            field: ColorU::from_u32(DARK_FIELD),
            plain_default: ColorU::from_u32(DARK_DEFAULT_TEXT),
            string_escape: ColorU::from_u32(DARK_STRING_ESCAPE),
            // Boolean literals, commas/semicolons and string-template `${}` delimiters all
            // inherit the keyword color in JetBrains' schemes.
            boolean: ColorU::from_u32(DARK_KEYWORD),
            punctuation_delimiter: ColorU::from_u32(DARK_KEYWORD),
            punctuation_bracket: ColorU::from_u32(DARK_DEFAULT_TEXT),
            annotation: ColorU::from_u32(DARK_DEFAULT_TEXT),
            namespace: ColorU::from_u32(DARK_DEFAULT_TEXT),
            operator: ColorU::from_u32(DARK_DEFAULT_TEXT),
            label: ColorU::from_u32(DARK_DEFAULT_TEXT),
        }
    } else {
        JbSyntaxPalette {
            keyword: ColorU::from_u32(LIGHT_KEYWORD),
            string: ColorU::from_u32(LIGHT_STRING),
            number: ColorU::from_u32(LIGHT_NUMBER),
            comment: ColorU::from_u32(LIGHT_COMMENT),
            doc_comment: ColorU::from_u32(LIGHT_DOC_COMMENT),
            function_declaration: ColorU::from_u32(LIGHT_FUNCTION_DECLARATION),
            function_call: ColorU::from_u32(LIGHT_DEFAULT_TEXT),
            field: ColorU::from_u32(LIGHT_FIELD),
            plain_default: ColorU::from_u32(LIGHT_DEFAULT_TEXT),
            string_escape: ColorU::from_u32(LIGHT_STRING_ESCAPE),
            boolean: ColorU::from_u32(LIGHT_KEYWORD),
            punctuation_delimiter: ColorU::from_u32(LIGHT_KEYWORD),
            punctuation_bracket: ColorU::from_u32(LIGHT_DEFAULT_TEXT),
            annotation: ColorU::from_u32(LIGHT_DEFAULT_TEXT),
            namespace: ColorU::from_u32(LIGHT_DEFAULT_TEXT),
            operator: ColorU::from_u32(LIGHT_DEFAULT_TEXT),
            label: ColorU::from_u32(LIGHT_DEFAULT_TEXT),
        }
    }
}

/// The syntax highlighting [`ColorMap`] matching the JetBrains scheme for the given theme.
pub(crate) fn syntax_color_map(color_scheme: ColorScheme) -> ColorMap {
    let palette = palette(color_scheme == ColorScheme::LightOnDark);
    ColorMap {
        keyword_color: palette.keyword,
        string_color: palette.string,
        number_color: palette.number,
        comment_color: palette.comment,
        doc_comment_color: palette.doc_comment,
        type_color: palette.plain_default,
        function_declaration_color: palette.function_declaration,
        function_call_color: palette.function_call,
        field_color: palette.field,
        plain_default_color: palette.plain_default,
        string_escape_color: palette.string_escape,
        boolean_color: palette.boolean,
        punctuation_delimiter_color: palette.punctuation_delimiter,
        punctuation_bracket_color: palette.punctuation_bracket,
        annotation_color: palette.annotation,
        namespace_color: palette.namespace,
        operator_color: palette.operator,
        label_color: palette.label,
    }
}

#[cfg(test)]
#[path = "jb_syntax_theme_tests.rs"]
mod tests;
