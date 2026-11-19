//! Reading the IDE's XML: `<option name=… value=…>` children, lists, maps and env entries.

use std::collections::BTreeMap;

use roxmltree::Node;

/// The element children of `node` with this tag name.
pub(crate) fn children<'a, 'input>(
    node: Node<'a, 'input>,
    tag: &'static str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children()
        .filter(move |child| child.is_element() && child.has_tag_name(tag))
}

pub(crate) fn child<'a, 'input>(
    node: Node<'a, 'input>,
    tag: &'static str,
) -> Option<Node<'a, 'input>> {
    children(node, tag).next()
}

/// The `<option name="name">` child of `node`.
pub(crate) fn option_node<'a, 'input>(
    node: Node<'a, 'input>,
    name: &str,
) -> Option<Node<'a, 'input>> {
    children(node, "option").find(|option| option.attribute("name") == Some(name))
}

/// The `value` of the `<option name="name">` child of `node`; an option without a value (or an
/// empty one) counts as absent.
pub(crate) fn option<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    option_node(node, name)
        .and_then(|option| option.attribute("value"))
        .filter(|value| !value.is_empty())
}

/// Whether the `<option name="name">` child of `node` is `true`, else `default`.
pub(crate) fn option_flag(node: Node<'_, '_>, name: &str, default: bool) -> bool {
    match option(node, name) {
        Some(value) => value == "true",
        None => default,
    }
}

/// The values of `<option name="name"><list><option value="…"/></list></option>`.
pub(crate) fn option_list(node: Node<'_, '_>, name: &str) -> Vec<String> {
    option_node(node, name)
        .and_then(|option| child(option, "list"))
        .map(|list| {
            children(list, "option")
                .filter_map(|item| item.attribute("value"))
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The entries of `<option name="name"><map><entry key="…" value="…"/></map></option>`.
pub(crate) fn option_map(node: Node<'_, '_>, name: &str) -> BTreeMap<String, String> {
    option_node(node, name)
        .and_then(|option| child(option, "map"))
        .map(|map| {
            children(map, "entry")
                .filter_map(|entry| {
                    Some((
                        entry.attribute("key")?.to_owned(),
                        entry.attribute("value").unwrap_or_default().to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The entries of `<envs><env name="…" value="…"/></envs>`.
pub(crate) fn envs(node: Node<'_, '_>) -> BTreeMap<String, String> {
    child(node, "envs")
        .map(|envs| {
            children(envs, "env")
                .filter_map(|env| {
                    Some((
                        env.attribute("name")?.to_owned(),
                        env.attribute("value").unwrap_or_default().to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The 1-based line of `node` in the document, for messages.
pub(crate) fn line_of(node: Node<'_, '_>) -> u32 {
    node.document().text_pos_at(node.range().start).row
}
