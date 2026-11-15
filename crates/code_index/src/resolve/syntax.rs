//! What a Kotlin or Java syntax tree says about a reference: what kind of name it is, which local
//! declarations and enclosing classes are in scope, and the declared types of variables.

use arborium_tree_sitter::Node;

use crate::extract::{child_of_kind, compact_text, named_children, text};
use crate::model::Language;

/// What kind of declaration a name may refer to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Want {
    Type,
    /// The callee of a call: a function, a constructor (through its class) or a function-typed
    /// property.
    Call,
    /// A name used as a value: a property, an enum entry, an object or a class.
    Value,
}

/// How the name under the cursor is looked up.
#[derive(Debug)]
pub(crate) enum Lookup<'tree> {
    /// A fully qualified name (an import directive up to the name).
    Qualified(String),
    /// An unqualified name, looked up in the scopes around it.
    Name(Want),
    /// `receiver.name`.
    Member { receiver: Node<'tree>, want: Want },
    /// A type name qualified by outer types or a package (`Outer.Inner`, `a.b.Type`).
    QualifiedType(Vec<String>),
    /// The name of the declaration it's in.
    Declaration,
    /// Not something declarations are found for (a package name, a named argument, …).
    Nothing,
}

/// A variable or parameter declared in a function body, lambda or parameter list.
pub(crate) struct LocalDecl<'tree> {
    pub name: Node<'tree>,
    /// The declared type, or the class a Kotlin `val x = Foo(…)` constructs.
    pub type_node: Option<TypeNode<'tree>>,
    /// A local function rather than a variable.
    pub is_function: bool,
    /// A Kotlin variable or parameter of a function type (`onClick: () -> Unit`), so callable.
    pub is_function_typed: bool,
}

/// A type as written: a type node, or the callee of a constructor call.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TypeNode<'tree> {
    Type(Node<'tree>),
    Constructed(Node<'tree>),
}

/// The shapes of receiver expression whose type is cheap to find.
pub(crate) enum Receiver<'tree> {
    Identifier(Node<'tree>),
    This,
    /// `receiver.name` itself used as a receiver (`Outer.Inner.make()`).
    Member {
        receiver: Node<'tree>,
        name: Node<'tree>,
    },
    /// A call of a name (`Foo(…).bar`); its type is the class if the name is one.
    Call(Node<'tree>),
    /// Java `new Foo(…)`.
    New(Node<'tree>),
    Other,
}

pub(crate) fn is_identifier(kind: &str) -> bool {
    matches!(
        kind,
        "simple_identifier" | "type_identifier" | "identifier" | "interpolated_identifier"
    )
}

/// The identifier's name: its text without Kotlin backticks.
pub(crate) fn identifier_name<'a>(node: Node, source: &'a [u8]) -> &'a str {
    text(node, source).trim_matches('`')
}

pub(crate) fn classify<'tree>(language: Language, id: Node<'tree>, source: &[u8]) -> Lookup<'tree> {
    match language {
        Language::Kotlin => classify_kotlin(id, source),
        Language::Java => classify_java(id, source),
    }
}

fn classify_kotlin<'tree>(id: Node<'tree>, source: &[u8]) -> Lookup<'tree> {
    let Some(parent) = id.parent() else {
        return Lookup::Nothing;
    };
    if id.kind() == "interpolated_identifier" {
        return Lookup::Name(Want::Value);
    }
    match parent.kind() {
        "identifier" => match parent.parent().map(|grand| grand.kind()) {
            Some("import_header") => {
                let start = parent.start_byte();
                Lookup::Qualified(compact(&source[start..id.end_byte()]))
            }
            _ => Lookup::Nothing,
        },
        "import_alias" => parent
            .parent()
            .and_then(|header| child_of_kind(header, "identifier"))
            .map_or(Lookup::Nothing, |path| {
                Lookup::Qualified(compact_text(path, source))
            }),
        "navigation_suffix" => {
            let Some(expression) = parent.parent() else {
                return Lookup::Nothing;
            };
            match expression.named_child(0) {
                Some(receiver) if receiver.id() != parent.id() => Lookup::Member {
                    receiver,
                    want: if is_callee(expression) {
                        Want::Call
                    } else {
                        Want::Value
                    },
                },
                _ => Lookup::Nothing,
            }
        }
        "call_expression" if is_first_named_child(parent, id) => Lookup::Name(Want::Call),
        // `Type::member`, `value::member`, `::member`.
        "callable_reference" => match parent.named_child(0) {
            Some(receiver) if receiver.id() != id.id() => Lookup::Member {
                receiver,
                want: Want::Call,
            },
            _ => Lookup::Name(Want::Call),
        },
        "user_type" => {
            let qualifier: Vec<String> = named_children(parent)
                .take_while(|child| child.id() != id.id())
                .filter(|child| child.kind() == "type_identifier")
                .map(|child| identifier_name(child, source).to_owned())
                .collect();
            if qualifier.is_empty() {
                Lookup::Name(Want::Type)
            } else {
                let mut path = qualifier;
                path.push(identifier_name(id, source).to_owned());
                Lookup::QualifiedType(path)
            }
        }
        "class_declaration"
        | "object_declaration"
        | "type_alias"
        | "function_declaration"
        | "variable_declaration"
        | "class_parameter"
        | "parameter"
        | "enum_entry"
        | "type_parameter"
        | "catch_block" => Lookup::Declaration,
        "package_header" | "this_expression" | "super_expression" | "label" => Lookup::Nothing,
        "value_argument"
            if id
                .next_sibling()
                .is_some_and(|next| !next.is_named() && text(next, source) == "=") =>
        {
            Lookup::Nothing
        }
        _ => Lookup::Name(Want::Value),
    }
}

fn classify_java<'tree>(id: Node<'tree>, source: &[u8]) -> Lookup<'tree> {
    let Some(parent) = id.parent() else {
        return Lookup::Nothing;
    };
    if let Some(import) = ancestor_of_kind(id, "import_declaration") {
        let start = named_children(import)
            .find(|node| matches!(node.kind(), "scoped_identifier" | "identifier"))
            .map_or(id.start_byte(), |name| name.start_byte());
        return Lookup::Qualified(compact(&source[start..id.end_byte()]));
    }
    if ancestor_of_kind(id, "package_declaration").is_some() {
        return Lookup::Nothing;
    }
    let field = field_name_of(parent, id);
    match (parent.kind(), field) {
        ("method_invocation", Some("name")) => match parent.child_by_field_name("object") {
            Some(receiver) => Lookup::Member {
                receiver,
                want: Want::Call,
            },
            None => Lookup::Name(Want::Call),
        },
        ("field_access", Some("field")) => match parent.child_by_field_name("object") {
            Some(receiver) => Lookup::Member {
                receiver,
                want: Want::Value,
            },
            None => Lookup::Nothing,
        },
        ("scoped_type_identifier" | "scoped_identifier", _) => {
            let mut path: Vec<String> = compact(&source[parent.start_byte()..id.start_byte()])
                .split('.')
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect();
            path.push(identifier_name(id, source).to_owned());
            if path.len() == 1 {
                Lookup::Name(Want::Type)
            } else {
                Lookup::QualifiedType(path)
            }
        }
        (
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "annotation_type_declaration"
            | "method_declaration"
            | "constructor_declaration"
            | "compact_constructor_declaration"
            | "variable_declarator"
            | "formal_parameter"
            | "catch_formal_parameter"
            | "enhanced_for_statement"
            | "enum_constant"
            | "resource"
            | "type_parameter",
            Some("name"),
        )
        | ("lambda_expression", Some("parameters"))
        | ("inferred_parameters" | "type_parameter", _) => Lookup::Declaration,
        _ if id.kind() == "type_identifier" => Lookup::Name(Want::Type),
        ("method_invocation", Some("object")) | ("field_access", Some("object")) => {
            Lookup::Name(Want::Value)
        }
        _ => Lookup::Name(Want::Value),
    }
}

fn compact(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn is_callee(expression: Node) -> bool {
    expression.parent().is_some_and(|parent| {
        parent.kind() == "call_expression" && is_first_named_child(parent, expression)
    })
}

fn is_first_named_child(parent: Node, child: Node) -> bool {
    parent
        .named_child(0)
        .is_some_and(|first| first.id() == child.id())
}

fn ancestor_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if ancestor.kind() == kind {
            return Some(ancestor);
        }
        current = ancestor.parent();
    }
    None
}

fn field_name_of(parent: Node, child: Node) -> Option<&'static str> {
    let mut cursor = parent.walk();
    for (index, candidate) in parent.children(&mut cursor).enumerate() {
        if candidate.id() == child.id() {
            return parent.field_name_for_child(index as u32);
        }
    }
    None
}

/// The innermost local variable, parameter or local function named `name` that is in scope at
/// `at`, stopping at the enclosing class.
pub(crate) fn find_local<'tree>(
    language: Language,
    name: &str,
    at: Node<'tree>,
    source: &[u8],
) -> Option<LocalDecl<'tree>> {
    let mut node = at;
    let mut in_function = false;
    while let Some(parent) = node.parent() {
        let found = match language {
            Language::Kotlin => kotlin_scope_local(parent, node, name, at, in_function, source),
            Language::Java => java_scope_local(parent, node, name, at, source),
        };
        in_function |= matches!(
            parent.kind(),
            "function_declaration" | "getter" | "setter" | "secondary_constructor"
        );
        match found {
            ScopeResult::Found(local) => return Some(local),
            ScopeResult::Stop => return None,
            ScopeResult::Continue => {}
        }
        node = parent;
    }
    None
}

enum ScopeResult<'tree> {
    Found(LocalDecl<'tree>),
    Continue,
    Stop,
}

fn named(name: &str, node: Node, source: &[u8]) -> bool {
    identifier_name(node, source) == name
}

fn variable<'tree>(
    name_node: Node<'tree>,
    type_node: Option<TypeNode<'tree>>,
) -> ScopeResult<'tree> {
    ScopeResult::Found(LocalDecl {
        name: name_node,
        type_node,
        is_function: false,
        is_function_typed: name_node.parent().is_some_and(has_function_type),
    })
}

/// Whether a declaration node's type is a Kotlin function type, possibly nullable.
fn has_function_type(declaration: Node) -> bool {
    named_children(declaration).any(|child| match child.kind() {
        "function_type" => true,
        "nullable_type" | "parenthesized_type" => named_children(child)
            .any(|inner| inner.kind() == "function_type" || has_function_type(inner)),
        _ => false,
    })
}

/// A Kotlin `variable_declaration`'s name and declared type.
fn kotlin_variable<'tree>(declaration: Node<'tree>) -> (Option<Node<'tree>>, Option<Node<'tree>>) {
    let name = child_of_kind(declaration, "simple_identifier");
    let type_node = named_children(declaration)
        .find(|child| matches!(child.kind(), "user_type" | "nullable_type"));
    (name, type_node)
}

fn kotlin_scope_local<'tree>(
    scope: Node<'tree>,
    child: Node<'tree>,
    name: &str,
    at: Node<'tree>,
    in_function: bool,
    source: &[u8],
) -> ScopeResult<'tree> {
    match scope.kind() {
        "statements" => {
            let mut found = ScopeResult::Continue;
            for statement in named_children(scope) {
                if statement.start_byte() >= at.start_byte() || statement.id() == child.id() {
                    break;
                }
                match statement.kind() {
                    "property_declaration" => {
                        if let Some(local) = kotlin_property_local(statement, name, source) {
                            found = local;
                        }
                    }
                    "function_declaration" => {
                        if let Some(fun_name) = child_of_kind(statement, "simple_identifier")
                            && named(name, fun_name, source)
                        {
                            found = ScopeResult::Found(LocalDecl {
                                name: fun_name,
                                type_node: None,
                                is_function: true,
                                is_function_typed: false,
                            });
                        }
                    }
                    _ => {}
                }
            }
            found
        }
        "function_declaration" | "anonymous_function" | "secondary_constructor" | "setter" => {
            let parameters = child_of_kind(scope, "function_value_parameters")
                .map(named_children)
                .into_iter()
                .flatten()
                .chain(named_children(scope).filter(|node| node.kind() == "parameter"));
            for parameter in parameters {
                if parameter.kind() != "parameter" {
                    continue;
                }
                if let Some(param_name) = child_of_kind(parameter, "simple_identifier")
                    && named(name, param_name, source)
                {
                    let type_node = named_children(parameter)
                        .find(|node| matches!(node.kind(), "user_type" | "nullable_type"))
                        .map(TypeNode::Type);
                    return variable(param_name, type_node);
                }
            }
            ScopeResult::Continue
        }
        "lambda_literal" => {
            if let Some(parameters) = child_of_kind(scope, "lambda_parameters") {
                for declaration in kotlin_declarations(parameters) {
                    let (Some(var_name), type_node) = kotlin_variable(declaration) else {
                        continue;
                    };
                    if named(name, var_name, source) {
                        return variable(var_name, type_node.map(TypeNode::Type));
                    }
                }
            }
            ScopeResult::Continue
        }
        "for_statement" | "when_subject" => {
            for declaration in kotlin_declarations(scope) {
                let (Some(var_name), type_node) = kotlin_variable(declaration) else {
                    continue;
                };
                if named(name, var_name, source) && declaration.id() != child.id() {
                    return variable(var_name, type_node.map(TypeNode::Type));
                }
            }
            ScopeResult::Continue
        }
        "catch_block" => match child_of_kind(scope, "simple_identifier") {
            Some(var_name) if named(name, var_name, source) => {
                let type_node = child_of_kind(scope, "user_type").map(TypeNode::Type);
                variable(var_name, type_node)
            }
            _ => ScopeResult::Continue,
        },
        "class_declaration" if in_function => ScopeResult::Stop,
        "class_declaration" => {
            // Plain constructor parameters (without `val`/`var`) are visible in initializers,
            // not in member functions.
            let parameters = child_of_kind(scope, "primary_constructor")
                .map(named_children)
                .into_iter()
                .flatten();
            for parameter in parameters {
                if parameter.kind() == "class_parameter"
                    && child_of_kind(parameter, "binding_pattern_kind").is_none()
                    && let Some(param_name) = child_of_kind(parameter, "simple_identifier")
                    && named(name, param_name, source)
                {
                    let type_node = named_children(parameter)
                        .find(|node| matches!(node.kind(), "user_type" | "nullable_type"))
                        .map(TypeNode::Type);
                    return variable(param_name, type_node);
                }
            }
            ScopeResult::Stop
        }
        "object_declaration" | "companion_object" | "source_file" => ScopeResult::Stop,
        _ => ScopeResult::Continue,
    }
}

/// `variable_declaration`s directly inside `node`, including destructured ones.
fn kotlin_declarations(node: Node) -> Vec<Node> {
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
    declarations
}

fn kotlin_property_local<'tree>(
    property: Node<'tree>,
    name: &str,
    source: &[u8],
) -> Option<ScopeResult<'tree>> {
    for declaration in kotlin_declarations(property) {
        let (Some(var_name), type_node) = kotlin_variable(declaration) else {
            continue;
        };
        if named(name, var_name, source) {
            let type_node = type_node
                .map(TypeNode::Type)
                .or_else(|| constructed_type(property));
            return Some(variable(var_name, type_node));
        }
    }
    None
}

/// The callee of `val x = Foo(…)` when it's capitalized like a class.
fn constructed_type(property: Node) -> Option<TypeNode> {
    let call = named_children(property).find(|child| child.kind() == "call_expression")?;
    let callee = call.named_child(0)?;
    (callee.kind() == "simple_identifier").then_some(TypeNode::Constructed(callee))
}

fn java_scope_local<'tree>(
    scope: Node<'tree>,
    child: Node<'tree>,
    name: &str,
    at: Node<'tree>,
    source: &[u8],
) -> ScopeResult<'tree> {
    match scope.kind() {
        "block" | "switch_block_statement_group" | "constructor_body" => {
            let mut found = ScopeResult::Continue;
            for statement in named_children(scope) {
                if statement.start_byte() >= at.start_byte() || statement.id() == child.id() {
                    break;
                }
                if statement.kind() == "local_variable_declaration"
                    && let Some(local) = java_declarators(statement, name, source)
                {
                    found = local;
                }
            }
            found
        }
        "for_statement" => named_children(scope)
            .filter(|node| node.kind() == "local_variable_declaration")
            .find_map(|declaration| java_declarators(declaration, name, source))
            .unwrap_or(ScopeResult::Continue),
        "method_declaration" | "constructor_declaration" => {
            let parameters = scope
                .child_by_field_name("parameters")
                .map(named_children)
                .into_iter()
                .flatten();
            for parameter in parameters {
                if matches!(parameter.kind(), "formal_parameter" | "spread_parameter")
                    && let Some(param_name) = parameter.child_by_field_name("name").or_else(|| {
                        child_of_kind(parameter, "variable_declarator")
                            .and_then(|declarator| declarator.child_by_field_name("name"))
                    })
                    && named(name, param_name, source)
                {
                    let type_node = parameter.child_by_field_name("type").map(TypeNode::Type);
                    return variable(param_name, type_node);
                }
            }
            ScopeResult::Stop
        }
        "lambda_expression" => {
            if let Some(parameters) = scope.child_by_field_name("parameters") {
                let candidates: Vec<Node> = if parameters.kind() == "identifier" {
                    vec![parameters]
                } else {
                    named_children(parameters)
                        .flat_map(|parameter| {
                            if parameter.kind() == "identifier" {
                                Some(parameter)
                            } else {
                                parameter.child_by_field_name("name")
                            }
                        })
                        .collect()
                };
                if let Some(param_name) = candidates
                    .into_iter()
                    .find(|node| named(name, *node, source))
                {
                    let type_node = param_name
                        .parent()
                        .and_then(|parameter| parameter.child_by_field_name("type"))
                        .map(TypeNode::Type);
                    return variable(param_name, type_node);
                }
            }
            ScopeResult::Continue
        }
        "enhanced_for_statement" => match scope.child_by_field_name("name") {
            Some(var_name) if named(name, var_name, source) => variable(
                var_name,
                scope.child_by_field_name("type").map(TypeNode::Type),
            ),
            _ => ScopeResult::Continue,
        },
        "catch_clause" => {
            if let Some(parameter) = child_of_kind(scope, "catch_formal_parameter")
                && let Some(var_name) = parameter.child_by_field_name("name")
                && named(name, var_name, source)
            {
                let type_node = child_of_kind(parameter, "catch_type")
                    .and_then(|types| types.named_child(0))
                    .map(TypeNode::Type);
                return variable(var_name, type_node);
            }
            ScopeResult::Continue
        }
        "try_with_resources_statement" => {
            let resources = scope
                .child_by_field_name("resources")
                .map(named_children)
                .into_iter()
                .flatten();
            for resource in resources {
                if let Some(var_name) = resource.child_by_field_name("name")
                    && named(name, var_name, source)
                {
                    return variable(
                        var_name,
                        resource.child_by_field_name("type").map(TypeNode::Type),
                    );
                }
            }
            ScopeResult::Continue
        }
        "class_body" | "interface_body" | "enum_body" | "program" => ScopeResult::Stop,
        _ => ScopeResult::Continue,
    }
}

fn java_declarators<'tree>(
    declaration: Node<'tree>,
    name: &str,
    source: &[u8],
) -> Option<ScopeResult<'tree>> {
    let type_node = declaration.child_by_field_name("type");
    named_children(declaration)
        .filter(|node| node.kind() == "variable_declarator")
        .filter_map(|declarator| declarator.child_by_field_name("name"))
        .find(|var_name| named(name, *var_name, source))
        .map(|var_name| {
            // `var x = new Foo()` declares no type; use the constructed class.
            let type_node = match type_node {
                Some(node) if text(node, source) != "var" => Some(TypeNode::Type(node)),
                _ => var_name
                    .parent()
                    .and_then(|declarator| declarator.child_by_field_name("value"))
                    .filter(|value| value.kind() == "object_creation_expression")
                    .and_then(|value| value.child_by_field_name("type"))
                    .map(TypeNode::Type),
            };
            variable(var_name, type_node)
        })
}

/// A class, interface, object or companion object enclosing a reference.
pub(crate) struct EnclosingClass<'tree> {
    pub name: String,
    pub node: Node<'tree>,
}

/// The classes enclosing `at`, outermost first.
pub(crate) fn enclosing_classes<'tree>(
    language: Language,
    at: Node<'tree>,
    source: &[u8],
) -> Vec<EnclosingClass<'tree>> {
    let mut classes = Vec::new();
    let mut current = at.parent();
    while let Some(node) = current {
        let name = match (language, node.kind()) {
            (Language::Kotlin, "class_declaration" | "object_declaration") => {
                child_of_kind(node, "type_identifier")
                    .map(|name| identifier_name(name, source).to_owned())
            }
            (Language::Kotlin, "companion_object") => {
                Some(child_of_kind(node, "type_identifier").map_or_else(
                    || "Companion".to_owned(),
                    |name| identifier_name(name, source).to_owned(),
                ))
            }
            (
                Language::Java,
                "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
                | "annotation_type_declaration",
            ) => node
                .child_by_field_name("name")
                .map(|name| text(name, source).to_owned()),
            _ => None,
        };
        if let Some(name) = name {
            classes.push(EnclosingClass { name, node });
        }
        current = node.parent();
    }
    classes.reverse();
    classes
}

/// The supertypes a class declares, as type nodes.
pub(crate) fn supertypes<'tree>(language: Language, class: Node<'tree>) -> Vec<Node<'tree>> {
    match language {
        Language::Kotlin => named_children(class)
            .filter(|child| child.kind() == "delegation_specifier")
            .filter_map(|specifier| {
                named_children(specifier).find_map(|child| match child.kind() {
                    "user_type" => Some(child),
                    "constructor_invocation" => child_of_kind(child, "user_type"),
                    _ => None,
                })
            })
            .collect(),
        Language::Java => {
            let mut types = Vec::new();
            if let Some(superclass) = class.child_by_field_name("superclass") {
                types.extend(superclass.named_child(0));
            }
            for field in ["interfaces", "extends_interfaces"] {
                if let Some(interfaces) = class
                    .child_by_field_name(field)
                    .or_else(|| child_of_kind(class, field))
                {
                    for list in named_children(interfaces) {
                        types.extend(named_children(list));
                    }
                }
            }
            types
        }
    }
}

/// The declared type of the class property or field named `name` in `class` (including Kotlin
/// constructor properties), or of the top-level property when `class` is the file.
pub(crate) fn property_type<'tree>(
    language: Language,
    class: Node<'tree>,
    name: &str,
    source: &[u8],
) -> Option<TypeNode<'tree>> {
    match language {
        Language::Kotlin => {
            let parameters = child_of_kind(class, "primary_constructor")
                .map(named_children)
                .into_iter()
                .flatten();
            for parameter in parameters {
                if parameter.kind() == "class_parameter"
                    && let Some(param_name) = child_of_kind(parameter, "simple_identifier")
                    && named(name, param_name, source)
                {
                    return named_children(parameter)
                        .find(|node| matches!(node.kind(), "user_type" | "nullable_type"))
                        .map(TypeNode::Type);
                }
            }
            let body = if class.kind() == "source_file" {
                Some(class)
            } else {
                child_of_kind(class, "class_body")
            }?;
            named_children(body)
                .filter(|member| member.kind() == "property_declaration")
                .find_map(
                    |property| match kotlin_property_local(property, name, source)? {
                        ScopeResult::Found(local) => local.type_node,
                        ScopeResult::Continue | ScopeResult::Stop => None,
                    },
                )
        }
        Language::Java => {
            let body = class.child_by_field_name("body")?;
            named_children(body)
                .filter(|member| {
                    matches!(member.kind(), "field_declaration" | "constant_declaration")
                })
                .find_map(|field| match java_declarators(field, name, source)? {
                    ScopeResult::Found(local) => local.type_node,
                    ScopeResult::Continue | ScopeResult::Stop => None,
                })
        }
    }
}

/// The dotted name of a type node without type arguments or nullability (`List<Foo>?` →
/// `["List"]`, `Outer.Inner` → `["Outer", "Inner"]`).
pub(crate) fn type_path(type_node: TypeNode, source: &[u8]) -> Vec<String> {
    let node = match type_node {
        TypeNode::Constructed(callee) => return vec![identifier_name(callee, source).to_owned()],
        TypeNode::Type(node) => node,
    };
    match node.kind() {
        "nullable_type" => node
            .named_child(0)
            .map(|inner| type_path(TypeNode::Type(inner), source))
            .unwrap_or_default(),
        "user_type" => named_children(node)
            .filter(|child| child.kind() == "type_identifier")
            .map(|child| identifier_name(child, source).to_owned())
            .collect(),
        "generic_type" => node
            .named_child(0)
            .map(|inner| type_path(TypeNode::Type(inner), source))
            .unwrap_or_default(),
        "type_identifier" | "simple_identifier" | "identifier" => {
            vec![identifier_name(node, source).to_owned()]
        }
        "scoped_type_identifier" => compact_text(node, source)
            .split('<')
            .next()
            .unwrap_or_default()
            .split('.')
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn receiver_shape<'tree>(language: Language, node: Node<'tree>) -> Receiver<'tree> {
    match (language, node.kind()) {
        (_, "simple_identifier" | "identifier") | (Language::Kotlin, "type_identifier") => {
            Receiver::Identifier(node)
        }
        (Language::Kotlin, "user_type") if node.named_child_count() == 1 => node
            .named_child(0)
            .map_or(Receiver::Other, |name| receiver_shape(language, name)),
        (Language::Kotlin, "this_expression") | (Language::Java, "this") => Receiver::This,
        (Language::Kotlin, "navigation_expression") => {
            let receiver = node.named_child(0);
            let name = node
                .named_child(1)
                .filter(|suffix| suffix.kind() == "navigation_suffix")
                .and_then(|suffix| child_of_kind(suffix, "simple_identifier"));
            match (receiver, name) {
                (Some(receiver), Some(name)) => Receiver::Member { receiver, name },
                _ => Receiver::Other,
            }
        }
        (Language::Java, "field_access") => {
            match (
                node.child_by_field_name("object"),
                node.child_by_field_name("field"),
            ) {
                (Some(receiver), Some(name)) => Receiver::Member { receiver, name },
                _ => Receiver::Other,
            }
        }
        (Language::Kotlin, "call_expression") => match node.named_child(0) {
            Some(callee) if callee.kind() == "simple_identifier" => Receiver::Call(callee),
            _ => Receiver::Other,
        },
        (Language::Java, "object_creation_expression") => node
            .child_by_field_name("type")
            .map_or(Receiver::Other, Receiver::New),
        (Language::Kotlin, "parenthesized_expression") => node
            .named_child(0)
            .map_or(Receiver::Other, |inner| receiver_shape(language, inner)),
        _ => Receiver::Other,
    }
}

/// The receiver as a dotted name (`com.example.Foo`), if it's only identifiers and dots.
pub(crate) fn dotted_name(node: Node, source: &[u8]) -> Option<String> {
    let text = compact_text(node, source);
    text.split('.')
        .all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        })
        .then_some(text)
}
