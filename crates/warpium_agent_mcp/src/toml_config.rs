//! MCP servers in Codex's `config.toml`: `[mcp_servers.<name>]` tables with `command` and `args`.
//! Edits keep the rest of the file (comments, order, formatting) as it is.

use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::Change;

/// A server's `command` and `args`.
pub struct TomlServer<'a> {
    pub command: &'a str,
    pub args: &'a [String],
}

/// Applies `edits` (`(name, Some(server))` sets, `(name, None)` removes) to the `mcp_servers`
/// table. Returns what changed per server and the new text, `None` when nothing changed. Fails,
/// leaving the file alone, when the config can't be read. Keys of a server's table other than
/// `command` and `args` (`env`, timeouts) are kept.
pub fn update_codex_servers(
    existing: Option<&str>,
    edits: &[(&str, Option<TomlServer<'_>>)],
) -> Result<(Vec<Change>, Option<String>), String> {
    let mut document: DocumentMut = existing
        .unwrap_or_default()
        .parse()
        .map_err(|err| format!("the config isn't valid TOML: {err}"))?;
    let has_servers = document.contains_key("mcp_servers");
    let removing_only = edits.iter().all(|(_, server)| server.is_none());
    if removing_only && !has_servers {
        return Ok((vec![Change::Absent; edits.len()], None));
    }
    if !has_servers {
        let mut table = Table::new();
        // Only `[mcp_servers.<name>]` headers, no empty `[mcp_servers]`.
        table.set_implicit(true);
        document.insert("mcp_servers", Item::Table(table));
    }
    let servers = document["mcp_servers"]
        .as_table_like_mut()
        .ok_or("`mcp_servers` in the config isn't a table")?;
    let mut changes = Vec::with_capacity(edits.len());
    for (name, server) in edits {
        let change = match server {
            None => match servers.remove(name) {
                Some(_) => Change::Removed,
                None => Change::Absent,
            },
            Some(server) => {
                let existed = servers.contains_key(name);
                let entry = servers
                    .entry(name)
                    .or_insert_with(|| Item::Table(Table::new()));
                let table = entry
                    .as_table_like_mut()
                    .ok_or_else(|| format!("`mcp_servers.{name}` in the config isn't a table"))?;
                let same_command =
                    table.get("command").and_then(Item::as_str) == Some(server.command);
                let current_args: Option<Vec<&str>> =
                    table.get("args").and_then(Item::as_array).map(|args| {
                        args.iter().filter_map(|arg| arg.as_str()).collect()
                    });
                let same_args = current_args.is_some_and(|args| {
                    args.len() == server.args.len()
                        && args.iter().zip(server.args).all(|(a, b)| *a == b)
                });
                if existed && same_command && same_args {
                    Change::Unchanged
                } else {
                    table.insert("command", value(server.command));
                    table.insert(
                        "args",
                        value(server.args.iter().map(String::as_str).collect::<Array>()),
                    );
                    if existed {
                        Change::Updated
                    } else {
                        Change::Added
                    }
                }
            }
        };
        changes.push(change);
    }
    if !changes.iter().any(|change| change.is_change()) {
        return Ok((changes, None));
    }
    Ok((changes, Some(document.to_string())))
}

#[cfg(test)]
#[path = "toml_config_tests.rs"]
mod tests;
