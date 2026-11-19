//! Declaration extraction from Kotlin and Java syntax trees.

mod java;
mod kotlin;

use std::cell::RefCell;
use std::collections::HashSet;

use arborium_tree_sitter::{Node, Parser, Tree};

use crate::lex::{for_each_identifier, is_keyword};
use crate::model::{Language, Position, SymbolKind, Visibility};
use crate::resolve::{TypeNode, supertypes, type_path};

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
    /// `(index in symbols, dotted name as written)` of every supertype a type declares.
    pub supertypes: Vec<(u32, String)>,
    /// Every distinct identifier used in the file's code, for finding usages, one per line.
    /// One string rather than one per identifier: a file has hundreds, and their allocations
    /// (made on parse threads, freed after interning) fragmented the heap of a cold build.
    pub identifiers: String,
}

thread_local! {
    static KOTLIN_PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
    static JAVA_PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Parses `source` and extracts its package, imports, declarations and identifiers. Parsers are
/// cached per thread. Returns `None` if the grammar can't be loaded or parsing is aborted.
pub(crate) fn extract(language: Language, source: &[u8]) -> Option<RawFile> {
    let tree = parse(language, source)?;
    let mut file = extract_from_tree(language, &tree, source);
    if let Ok(text) = std::str::from_utf8(source) {
        let mut seen: HashSet<&str> = HashSet::new();
        for_each_identifier(text, language, |name, _| {
            if !is_keyword(name, language) && seen.insert(name) {
                file.identifiers.push_str(name);
                file.identifiers.push('\n');
            }
        });
    }
    Some(file)
}

/// The package, imports and declarations of an already parsed file (no identifiers).
pub(crate) fn extract_from_tree(language: Language, tree: &Tree, source: &[u8]) -> RawFile {
    let mut file = RawFile::default();
    match language {
        Language::Kotlin => kotlin::extract(tree.root_node(), source, &mut file),
        Language::Java => java::extract(tree.root_node(), source, &mut file),
    }
    file
}

/// Parses `source` with the thread's cached parser for `language`.
pub(crate) fn parse(language: Language, source: &[u8]) -> Option<Tree> {
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
        slot.as_mut()?.parse(source, None)
    })
}

pub(crate) fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    std::str::from_utf8(&source[node.byte_range()]).unwrap_or("")
}

/// The node's text with all whitespace removed, for dotted identifiers that may span lines.
pub(crate) fn compact_text(node: Node, source: &[u8]) -> String {
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

/// Records the supertypes the type declaration `node` declares, for the symbol at `symbol`.
fn push_supertypes(
    language: Language,
    node: Node,
    symbol: usize,
    source: &[u8],
    out: &mut Vec<(u32, String)>,
) {
    for type_node in supertypes(language, node) {
        let path = type_path(TypeNode::Type(type_node), source);
        if !path.is_empty() {
            out.push((symbol as u32, path.join(".")));
        }
    }
}

/// Indexed child access is linear in tree-sitter, so children are collected with a cursor.
pub(crate) fn children(node: Node) -> impl Iterator<Item = Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect::<Vec<_>>().into_iter()
}

pub(crate) fn named_children(node: Node) -> impl Iterator<Item = Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .collect::<Vec<_>>()
        .into_iter()
}

pub(crate) fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    named_children(node).find(|child| child.kind() == kind)
}

fn has_token(node: Node, token: &str) -> bool {
    children(node).any(|child| !child.is_named() && child.kind() == token)
}

/// Where `node` starts, with the column in characters (editors count characters, tree-sitter
/// counts bytes).
pub(crate) fn position(node: Node, source: &[u8]) -> Position {
    let point = node.start_position();
    let start = node.start_byte();
    let line_prefix = &source[start.saturating_sub(point.column)..start];
    Position {
        line: point.row as u32,
        column: String::from_utf8_lossy(line_prefix).chars().count() as u32,
    }
}

pub(crate) fn join_container(container: &str, name: &str) -> String {
    if container.is_empty() {
        name.to_owned()
    } else {
        format!("{container}.{name}")
    }
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
