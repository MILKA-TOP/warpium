use arborium_tree_sitter::Node;

use super::{
    RawFile, RawImport, RawSymbol, child_of_kind, children, compact_text, has_token,
    join_container, named_children, normalized_text, position, text,
};
use crate::model::{SymbolKind, Visibility, flags};

pub(super) fn extract(root: Node, source: &[u8], file: &mut RawFile) {
    for child in named_children(root) {
        match child.kind() {
            "package_header" => {
                if let Some(identifier) = child_of_kind(child, "identifier") {
                    file.package = compact_text(identifier, source);
                }
            }
            "import_list" => {
                for header in named_children(child) {
                    if header.kind() == "import_header" {
                        push_import(header, source, file);
                    }
                }
            }
            "import_header" => push_import(child, source, file),
            _ => {}
        }
    }
    visit_declarations(root, "", source, &mut file.symbols);
}

fn push_import(header: Node, source: &[u8], file: &mut RawFile) {
    let Some(identifier) = child_of_kind(header, "identifier") else {
        return;
    };
    let alias = child_of_kind(header, "import_alias")
        .and_then(|alias| child_of_kind(alias, "type_identifier"))
        .map(|name| text(name, source).to_owned());
    file.imports.push(RawImport {
        path: compact_text(identifier, source),
        alias,
        wildcard: child_of_kind(header, "wildcard_import").is_some(),
        is_static: false,
    });
}

/// Visits the declarations directly inside `parent` (a file or a class body). Function bodies
/// aren't entered, so local declarations are left out.
fn visit_declarations(parent: Node, container: &str, source: &[u8], out: &mut Vec<RawSymbol>) {
    for node in named_children(parent) {
        match node.kind() {
            "class_declaration" => visit_class(node, container, source, out),
            "object_declaration" => {
                visit_object(node, SymbolKind::Object, container, source, out);
            }
            "companion_object" => {
                visit_object(node, SymbolKind::CompanionObject, container, source, out);
            }
            "function_declaration" => {
                if let Some(name) = child_of_kind(node, "simple_identifier") {
                    let (visibility, flags) = modifiers(node, source);
                    out.push(symbol(
                        node,
                        name,
                        SymbolKind::Function,
                        visibility,
                        flags,
                        container,
                        receiver(node, source),
                        source,
                    ));
                }
            }
            "property_declaration" => visit_property(node, container, source, out),
            "type_alias" => {
                if let Some(name) = child_of_kind(node, "type_identifier") {
                    let (visibility, flags) = modifiers(node, source);
                    out.push(symbol(
                        node,
                        name,
                        SymbolKind::TypeAlias,
                        visibility,
                        flags,
                        container,
                        None,
                        source,
                    ));
                }
            }
            "secondary_constructor" => {
                let class_name = container.rsplit('.').next().unwrap_or_default();
                if !class_name.is_empty() {
                    let (visibility, flags) = modifiers(node, source);
                    out.push(RawSymbol {
                        name: class_name.to_owned(),
                        kind: SymbolKind::Constructor,
                        visibility,
                        flags,
                        container: container.to_owned(),
                        receiver: None,
                        name_pos: position(node, source),
                        start_line: node.start_position().row as u32,
                        end_line: node.end_position().row as u32,
                    });
                }
            }
            "enum_entry" => {
                if let Some(name) = child_of_kind(node, "simple_identifier") {
                    out.push(symbol(
                        node,
                        name,
                        SymbolKind::EnumEntry,
                        Visibility::Public,
                        0,
                        container,
                        None,
                        source,
                    ));
                }
            }
            // Error recovery sometimes wraps valid declarations (for example the members of a
            // `fun interface`), so look inside.
            "ERROR" => visit_declarations(node, container, source, out),
            _ => {}
        }
    }
}

fn visit_class(node: Node, container: &str, source: &[u8], out: &mut Vec<RawSymbol>) {
    let Some(name) = child_of_kind(node, "type_identifier") else {
        return;
    };
    let (visibility, mut symbol_flags) = modifiers(node, source);
    let class_modifiers = class_modifiers(node, source);
    let kind = if has_token(node, "interface") {
        SymbolKind::Interface
    } else if has_token(node, "enum") {
        SymbolKind::Enum
    } else if class_modifiers.contains(&"annotation") {
        SymbolKind::Annotation
    } else {
        SymbolKind::Class
    };
    if class_modifiers.contains(&"data") {
        symbol_flags |= flags::DATA;
    }
    if class_modifiers.contains(&"sealed") {
        symbol_flags |= flags::SEALED;
    }
    let symbol = symbol(
        node,
        name,
        kind,
        visibility,
        symbol_flags,
        container,
        None,
        source,
    );
    let inner = join_container(container, &symbol.name);
    out.push(symbol);

    if let Some(constructor) = child_of_kind(node, "primary_constructor") {
        for parameter in named_children(constructor) {
            if parameter.kind() != "class_parameter"
                || child_of_kind(parameter, "binding_pattern_kind").is_none()
            {
                continue;
            }
            if let Some(param_name) = child_of_kind(parameter, "simple_identifier") {
                let (visibility, flags) = modifiers(parameter, source);
                out.push(self::symbol(
                    parameter,
                    param_name,
                    SymbolKind::Property,
                    visibility,
                    flags,
                    &inner,
                    None,
                    source,
                ));
            }
        }
    }
    for body in named_children(node) {
        if matches!(body.kind(), "class_body" | "enum_class_body") {
            visit_declarations(body, &inner, source, out);
        }
    }
}

fn visit_object(
    node: Node,
    kind: SymbolKind,
    container: &str,
    source: &[u8],
    out: &mut Vec<RawSymbol>,
) {
    let name_node = child_of_kind(node, "type_identifier");
    let (visibility, flags) = modifiers(node, source);
    let symbol = match name_node {
        Some(name) => symbol(node, name, kind, visibility, flags, container, None, source),
        // An unnamed companion object is `Companion`; point at the `companion` keyword.
        None if kind == SymbolKind::CompanionObject => RawSymbol {
            name: "Companion".to_owned(),
            kind,
            visibility,
            flags,
            container: container.to_owned(),
            receiver: None,
            name_pos: children(node)
                .find(|child| child.kind() == "companion")
                .map(|keyword| position(keyword, source))
                .unwrap_or_else(|| position(node, source)),
            start_line: node.start_position().row as u32,
            end_line: node.end_position().row as u32,
        },
        None => return,
    };
    let inner = join_container(container, &symbol.name);
    out.push(symbol);
    if let Some(body) = child_of_kind(node, "class_body") {
        visit_declarations(body, &inner, source, out);
    }
}

fn visit_property(node: Node, container: &str, source: &[u8], out: &mut Vec<RawSymbol>) {
    let (visibility, flags) = modifiers(node, source);
    let receiver = receiver(node, source);
    let mut declarations = Vec::new();
    for child in named_children(node) {
        match child.kind() {
            "variable_declaration" => declarations.push(child),
            "multi_variable_declaration" => declarations.extend(
                named_children(child).filter(|child| child.kind() == "variable_declaration"),
            ),
            _ => {}
        }
    }
    for declaration in declarations {
        if let Some(name) = child_of_kind(declaration, "simple_identifier") {
            let mut symbol = symbol(
                node,
                name,
                SymbolKind::Property,
                visibility,
                flags,
                container,
                receiver.clone(),
                source,
            );
            symbol.name_pos = position(name, source);
            out.push(symbol);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn symbol(
    node: Node,
    name: Node,
    kind: SymbolKind,
    visibility: Visibility,
    flags: u8,
    container: &str,
    receiver: Option<String>,
    source: &[u8],
) -> RawSymbol {
    RawSymbol {
        name: text(name, source).trim_matches('`').to_owned(),
        kind,
        visibility,
        flags,
        container: container.to_owned(),
        receiver,
        name_pos: position(name, source),
        start_line: node.start_position().row as u32,
        end_line: node.end_position().row as u32,
    }
}

fn receiver(node: Node, source: &[u8]) -> Option<String> {
    let receiver = node.child_by_field_name("receiver")?;
    Some(normalized_text(receiver, source))
}

/// Visibility and `expect`/`actual`/`const` flags from the declaration's `modifiers`.
fn modifiers(node: Node, source: &[u8]) -> (Visibility, u8) {
    let mut visibility = Visibility::Public;
    let mut symbol_flags = 0;
    let Some(modifiers) = child_of_kind(node, "modifiers") else {
        return (visibility, symbol_flags);
    };
    for modifier in named_children(modifiers) {
        match modifier.kind() {
            "visibility_modifier" => {
                visibility = match text(modifier, source) {
                    "private" => Visibility::Private,
                    "protected" => Visibility::Protected,
                    "internal" => Visibility::Internal,
                    _ => Visibility::Public,
                };
            }
            "platform_modifier" => match text(modifier, source) {
                "expect" => symbol_flags |= flags::EXPECT,
                "actual" => symbol_flags |= flags::ACTUAL,
                _ => {}
            },
            "property_modifier" if text(modifier, source) == "const" => {
                symbol_flags |= flags::CONST;
            }
            _ => {}
        }
    }
    (visibility, symbol_flags)
}

fn class_modifiers<'a>(node: Node, source: &'a [u8]) -> Vec<&'a str> {
    child_of_kind(node, "modifiers")
        .map(|modifiers| {
            named_children(modifiers)
                .filter(|modifier| modifier.kind() == "class_modifier")
                .map(|modifier| text(modifier, source))
                .collect()
        })
        .unwrap_or_default()
}
