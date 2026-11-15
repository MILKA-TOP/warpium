//! Declaration extraction from Kotlin and Java syntax trees.

mod java;
mod kotlin;

use std::cell::RefCell;

use arborium_tree_sitter::{Node, Parser};

use crate::model::{Language, Position, SymbolKind, Visibility};

/// A declaration before its strings are interned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    pub flags: u8,
    pub container: String,
    pub receiver: Option<String>,
    pub name_pos: Position,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawImport {
    pub path: String,
    pub alias: Option<String>,
    pub wildcard: bool,
    pub is_static: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RawFile {
    pub package: String,
    pub imports: Vec<RawImport>,
    pub symbols: Vec<RawSymbol>,
}

thread_local! {
    static KOTLIN_PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
    static JAVA_PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Parses `source` and extracts its package, imports and declarations. Parsers are cached per
/// thread. Returns `None` if the grammar can't be loaded or parsing is aborted.
pub(crate) fn extract(language: Language, source: &[u8]) -> Option<RawFile> {
    let parser_cell = match language {
        Language::Kotlin => &KOTLIN_PARSER,
        Language::Java => &JAVA_PARSER,
    };
    parser_cell.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let mut parser = Parser::new();
            let grammar = match language {
                Language::Kotlin => arborium_kotlin::language(),
                Language::Java => arborium_java::language(),
            };
            parser.set_language(&grammar.into()).ok()?;
            *slot = Some(parser);
        }
        let parser = slot.as_mut()?;
        let tree = parser.parse(source, None)?;
        let mut file = RawFile::default();
        match language {
            Language::Kotlin => kotlin::extract(tree.root_node(), source, &mut file),
            Language::Java => java::extract(tree.root_node(), source, &mut file),
        }
        Some(file)
    })
}

fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    std::str::from_utf8(&source[node.byte_range()]).unwrap_or("")
}

/// The node's text with all whitespace removed, for dotted identifiers that may span lines.
fn compact_text(node: Node, source: &[u8]) -> String {
    text(node, source)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// The node's text with whitespace runs collapsed to single spaces.
fn normalized_text(node: Node, source: &[u8]) -> String {
    text(node, source)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Indexed child access is linear in tree-sitter, so children are collected with a cursor.
fn children(node: Node) -> impl Iterator<Item = Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect::<Vec<_>>().into_iter()
}

fn named_children(node: Node) -> impl Iterator<Item = Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .collect::<Vec<_>>()
        .into_iter()
}

fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    named_children(node).find(|child| child.kind() == kind)
}

fn has_token(node: Node, token: &str) -> bool {
    children(node).any(|child| !child.is_named() && child.kind() == token)
}

/// Where `node` starts, with the column in characters (editors count characters, tree-sitter
/// counts bytes).
fn position(node: Node, source: &[u8]) -> Position {
    let point = node.start_position();
    let start = node.start_byte();
    let line_prefix = &source[start.saturating_sub(point.column)..start];
    Position {
        line: point.row as u32,
        column: String::from_utf8_lossy(line_prefix).chars().count() as u32,
    }
}

fn join_container(container: &str, name: &str) -> String {
    if container.is_empty() {
        name.to_owned()
    } else {
        format!("{container}.{name}")
    }
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
