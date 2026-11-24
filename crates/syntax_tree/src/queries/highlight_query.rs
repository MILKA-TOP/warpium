use std::iter;
use std::ops::Range;

use arborium::tree_sitter::{Node, Query, QueryCursor, TextProvider, Tree};
use rangemap::RangeMap;
use streaming_iterator::StreamingIterator;
use string_offset::{ByteOffset, CharOffset};
use warp_editor::content::buffer::{Buffer, ToBufferByteOffset, ToBufferCharOffset};
use warp_editor::content::text::Bytes;
use warpui_core::color::ColorU;

/// Color mapping from parsed syntax token name to its corresponding highlighting color.
///
/// The fields follow the semantic roles of JetBrains' default IDE color schemes
/// (IntelliJ Light / new-UI Dark): roles the schemes leave at the default text color
/// (`plain_default`, operators, brackets, annotations, namespaces, labels) get their own
/// field so themes can still customize them.
#[derive(Clone, Copy)]
pub struct ColorMap {
    pub keyword_color: ColorU,
    pub string_color: ColorU,
    pub number_color: ColorU,
    pub comment_color: ColorU,
    pub doc_comment_color: ColorU,
    pub type_color: ColorU,
    pub function_declaration_color: ColorU,
    pub function_call_color: ColorU,
    pub field_color: ColorU,
    pub plain_default_color: ColorU,
    pub string_escape_color: ColorU,
    pub boolean_color: ColorU,
    pub punctuation_delimiter_color: ColorU,
    pub punctuation_bracket_color: ColorU,
    pub annotation_color: ColorU,
    pub namespace_color: ColorU,
    pub operator_color: ColorU,
    pub label_color: ColorU,
}

/// Query for retrieving syntax highlighting information on the tokens.
pub struct HighlightQuery {
    highlight_map: Vec<Option<ColorU>>,
}

impl HighlightQuery {
    pub fn new(query: &Query, color_map: ColorMap) -> Self {
        let highlight_map = query
            .capture_names()
            .iter()
            .map(|name| convert_capture_name_to_color(name, &color_map))
            .collect();

        Self { highlight_map }
    }

    /// Given the a character range, return its corresponding highlight colors.
    pub fn get_highlighted_chunks(
        &self,
        range: Range<CharOffset>,
        query: &Query,
        buffer: &Buffer,
        tree: &Tree,
    ) -> RangeMap<CharOffset, ColorU> {
        let mut range_map = RangeMap::new();

        let mut cursor = QueryCursor::new();
        let byte_start = range.start.to_buffer_byte_offset(buffer).as_usize();
        let byte_end = range.end.to_buffer_byte_offset(buffer).as_usize();
        cursor.set_byte_range(byte_start..byte_end);
        let mut captures = cursor.captures(query, tree.root_node(), TextBuffer(buffer));

        while let Some(matches) = captures.next() {
            for cap in matches.0.captures {
                let insertion_range = cap.node.byte_range();
                let color = self
                    .highlight_map
                    .get(cap.index as usize)
                    .and_then(|inner| *inner);

                if let Some(color) = color {
                    let char_start =
                        ByteOffset::from(insertion_range.start).to_buffer_char_offset(buffer);
                    let char_end =
                        ByteOffset::from(insertion_range.end).to_buffer_char_offset(buffer);
                    if char_start < char_end {
                        range_map.insert(char_start..char_end, color);
                    }
                }
            }
        }

        range_map
    }
}

fn convert_capture_name_to_color(name: &str, color_map: &ColorMap) -> Option<ColorU> {
    match name {
        // Markdown (`text.*` captures from the arborium markdown query).
        "text.title" => Some(color_map.keyword_color),
        "text.literal" => Some(color_map.string_color),
        "text.uri" => Some(color_map.function_declaration_color),
        "text.reference" => Some(color_map.field_color),

        // Keywords; JetBrains' default schemes color every one of these as keywords.
        "keyword"
        | "keyword.conditional"
        | "keyword.conditional.ternary"
        | "keyword.coroutine"
        | "keyword.directive"
        | "keyword.exception"
        | "keyword.function"
        | "keyword.import"
        | "keyword.modifier"
        | "keyword.operator"
        | "keyword.repeat"
        | "keyword.return"
        | "keyword.type"
        | "conditional"
        | "repeat"
        | "exception"
        | "storageclass"
        | "preproc" => Some(color_map.keyword_color),

        // Functions. The tree-sitter queries cannot tell a declaration from a call, so
        // `function`-style captures take the declaration color (the JetBrains default);
        // only explicitly call/builtin captures take the call color.
        "function" | "function.definition" | "function.macro" | "function.method" | "method" => {
            Some(color_map.function_declaration_color)
        }
        "function.builtin"
        | "function.call"
        | "function.macro.builtin"
        | "function.method.builtin"
        | "function.special"
        | "method.call" => Some(color_map.function_call_color),

        // Types. JetBrains' defaults leave type references and constructors at the default
        // text color, so themes set `type_color` to that unless they customize it.
        "type" | "type.builtin" | "type.definition" | "type.qualifier" => {
            Some(color_map.type_color)
        }
        "constructor" => Some(color_map.plain_default_color),

        // Variables, parameters and constants. JetBrains colors locals, parameters and
        // `this`/`super` as default text; fields, properties and constants get the field
        // color; boolean/null literals get the keyword color (Java's query captures them
        // as `constant.builtin`).
        "variable" | "variable.builtin" | "variable.parameter" | "parameter"
        | "parameter.builtin" => Some(color_map.plain_default_color),
        "property"
        | "property.definition"
        | "field"
        | "variable.member"
        | "constant"
        | "constant.macro" => Some(color_map.field_color),
        "boolean" | "constant.builtin" => Some(color_map.boolean_color),
        "label" => Some(color_map.label_color),

        // Packages and modules.
        "namespace" | "module" | "module.builtin" => Some(color_map.namespace_color),

        // Literals.
        "string"
        | "string.regex"
        | "string.regexp"
        | "string.special"
        | "string.special.key"
        | "string.special.path"
        | "string.special.regex"
        | "string.special.symbol"
        | "string.special.uri"
        | "character" => Some(color_map.string_color),
        "string.escape" | "escape" | "character.special" => Some(color_map.string_escape_color),
        "number" | "number.float" | "float" => Some(color_map.number_color),

        // Comments.
        "comment" | "comment.unused" => Some(color_map.comment_color),
        "comment.doc" | "comment.documentation" | "comment.doc.__attribute__" => {
            Some(color_map.doc_comment_color)
        }

        // Operators and punctuation.
        "operator" => Some(color_map.operator_color),
        "punctuation" | "punctuation.bracket" => Some(color_map.punctuation_bracket_color),
        // Commas and semicolons inherit the keyword color in JetBrains' schemes.
        "punctuation.delimiter" => Some(color_map.punctuation_delimiter_color),
        // String-template `${...}` delimiters are keywords in JetBrains' schemes.
        "punctuation.special" => Some(color_map.keyword_color),
        "attribute" => Some(color_map.annotation_color),
        "tag" | "tag.attribute" | "tag.error" => Some(color_map.type_color),

        _ => None,
    }
}

// The default tree-sitter implementation here is unsafe (since the cursor could query invalid ranges outside of content length).
// TODO(kevin): Once we migrate buffer to store ArrayStrings. We should implement the chunks API on buffer directly to avoid collecting
// into a String and then chunking them again for highlighting.
pub struct TextSlice<'a>(pub &'a [u8]);

impl TextSlice<'_> {
    fn get(&self, range: Range<usize>) -> Self {
        Self(self.0.get(range).unwrap_or_default())
    }
}

impl AsRef<[u8]> for TextSlice<'_> {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

impl<'a> TextProvider<TextSlice<'a>> for TextSlice<'a> {
    type I = iter::Once<TextSlice<'a>>;

    fn text(&mut self, node: Node) -> Self::I {
        iter::once(self.get(node.byte_range()))
    }
}

pub struct TextBuffer<'a>(pub &'a Buffer);

impl<'a> TextProvider<&'a [u8]> for TextBuffer<'a> {
    type I = Bytes<'a>;

    fn text(&mut self, node: Node) -> Self::I {
        let range = node.range();
        self.0.bytes_in_range(
            ByteOffset::from(range.start_byte),
            ByteOffset::from(range.end_byte),
        )
    }
}

#[cfg(test)]
#[path = "highlight_query_tests.rs"]
mod tests;
