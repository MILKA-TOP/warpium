use arborium_tree_sitter::Node;

use super::{
    RawFile, RawImport, RawSymbol, child_of_kind, children, compact_text, join_container,
    named_children, position, text,
};
use crate::model::{SymbolKind, Visibility, flags};

pub(super) fn extract(root: Node, source: &[u8], file: &mut RawFile) {
    for child in named_children(root) {
        match child.kind() {
            "package_declaration" => {
                if let Some(name) = named_children(child)
                    .find(|node| matches!(node.kind(), "scoped_identifier" | "identifier"))
                {
                    file.package = compact_text(name, source);
                }
            }
            "import_declaration" => {
                let Some(name) = named_children(child)
                    .find(|node| matches!(node.kind(), "scoped_identifier" | "identifier"))
                else {
                    continue;
                };
                file.imports.push(RawImport {
                    path: compact_text(name, source),
                    alias: None,
                    wildcard: child_of_kind(child, "asterisk").is_some(),
                    is_static: children(child).any(|node| node.kind() == "static"),
                });
            }
            _ => {}
        }
    }
    visit_members(root, "", false, source, &mut file.symbols);
}

fn type_kind(kind: &str) -> Option<SymbolKind> {
    match kind {
        "class_declaration" => Some(SymbolKind::Class),
        "interface_declaration" => Some(SymbolKind::Interface),
        "enum_declaration" => Some(SymbolKind::Enum),
        "annotation_type_declaration" => Some(SymbolKind::Annotation),
        "record_declaration" => Some(SymbolKind::Record),
        _ => None,
    }
}

/// Visits the declarations directly inside `parent` (a file or a type body). Method bodies aren't
/// entered, so local and anonymous classes are left out.
fn visit_members(
    parent: Node,
    container: &str,
    in_interface: bool,
    source: &[u8],
    out: &mut Vec<RawSymbol>,
) {
    // Interface members are implicitly public; everything else defaults to package-private.
    let default_visibility = if in_interface {
        Visibility::Public
    } else {
        Visibility::PackagePrivate
    };
    for node in named_children(parent) {
        let node_kind = node.kind();
        if let Some(kind) = type_kind(node_kind) {
            visit_type(node, kind, container, default_visibility, source, out);
            continue;
        }
        match node_kind {
            "method_declaration" | "annotation_type_element_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    out.push(symbol(
                        node,
                        name,
                        SymbolKind::Function,
                        default_visibility,
                        container,
                        source,
                    ));
                }
            }
            "constructor_declaration" | "compact_constructor_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    out.push(symbol(
                        node,
                        name,
                        SymbolKind::Constructor,
                        default_visibility,
                        container,
                        source,
                    ));
                }
            }
            "field_declaration" | "constant_declaration" => {
                for declarator in named_children(node) {
                    if declarator.kind() != "variable_declarator" {
                        continue;
                    }
                    if let Some(name) = declarator.child_by_field_name("name") {
                        out.push(symbol(
                            node,
                            name,
                            SymbolKind::Property,
                            default_visibility,
                            container,
                            source,
                        ));
                    }
                }
            }
            "enum_constant" => {
                if let Some(name) = node.child_by_field_name("name") {
                    out.push(symbol(
                        node,
                        name,
                        SymbolKind::EnumEntry,
                        Visibility::Public,
                        container,
                        source,
                    ));
                }
            }
            "enum_body_declarations" | "ERROR" => {
                visit_members(node, container, in_interface, source, out);
            }
            _ => {}
        }
    }
}

fn visit_type(
    node: Node,
    kind: SymbolKind,
    container: &str,
    default_visibility: Visibility,
    source: &[u8],
    out: &mut Vec<RawSymbol>,
) {
    let Some(name) = node.child_by_field_name("name") else {
        return;
    };
    let symbol = symbol(node, name, kind, default_visibility, container, source);
    let inner = join_container(container, &symbol.name);
    out.push(symbol);

    if kind == SymbolKind::Record
        && let Some(parameters) = node.child_by_field_name("parameters")
    {
        for parameter in named_children(parameters) {
            if let Some(param_name) = parameter.child_by_field_name("name") {
                out.push(self::symbol(
                    parameter,
                    param_name,
                    SymbolKind::Property,
                    Visibility::Private,
                    &inner,
                    source,
                ));
            }
        }
    }
    if let Some(body) = node.child_by_field_name("body") {
        let in_interface = matches!(kind, SymbolKind::Interface | SymbolKind::Annotation);
        visit_members(body, &inner, in_interface, source, out);
    }
}

fn symbol(
    node: Node,
    name: Node,
    kind: SymbolKind,
    default_visibility: Visibility,
    container: &str,
    source: &[u8],
) -> RawSymbol {
    let (visibility, symbol_flags) = modifiers(node, default_visibility);
    RawSymbol {
        name: text(name, source).to_owned(),
        kind,
        visibility,
        flags: symbol_flags,
        container: container.to_owned(),
        receiver: None,
        name_pos: position(name),
        start_line: node.start_position().row as u32,
        end_line: node.end_position().row as u32,
    }
}

fn modifiers(node: Node, default_visibility: Visibility) -> (Visibility, u8) {
    let mut visibility = default_visibility;
    let mut symbol_flags = 0;
    if let Some(modifiers) = child_of_kind(node, "modifiers") {
        for modifier in children(modifiers) {
            match modifier.kind() {
                "public" => visibility = Visibility::Public,
                "protected" => visibility = Visibility::Protected,
                "private" => visibility = Visibility::Private,
                "static" => symbol_flags |= flags::STATIC,
                _ => {}
            }
        }
    }
    (visibility, symbol_flags)
}
