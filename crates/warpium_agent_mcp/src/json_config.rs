//! MCP servers in JSON configs: `{"mcpServers": {...}}` maps (Warp's `.mcp.json`, Kimi Code's
//! `mcp.json`, arc's `<layer>/mcp.json`) and Claude Code's `~/.claude.json`, whose local-scope
//! servers live under `projects.<project path>.mcpServers`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::Change;

/// Parses a config, treating a missing or blank file as empty.
fn parse_config(existing: Option<&str>) -> Result<Map<String, Value>, String> {
    match existing.map(str::trim).filter(|text| !text.is_empty()) {
        None => Ok(Map::new()),
        Some(text) => match serde_json::from_str(text) {
            Ok(Value::Object(config)) => Ok(config),
            Ok(_) => Err("the config isn't a JSON object".to_owned()),
            Err(err) => Err(format!("the config isn't valid JSON: {err}")),
        },
    }
}

fn render(config: Map<String, Value>) -> Result<String, String> {
    let mut text = serde_json::to_string_pretty(&Value::Object(config))
        .map_err(|err| format!("couldn't write the config: {err}"))?;
    text.push('\n');
    Ok(text)
}

/// The servers map of `config`, in the wrapper forms Warp reads (`mcpServers`, `servers`,
/// `mcp.servers`), or the config itself when it is a bare map of servers. An empty config gets
/// the common `mcpServers` wrapper.
fn servers_map(config: &mut Map<String, Value>) -> &mut Map<String, Value> {
    let key = if config.get("mcpServers").is_some_and(Value::is_object) || config.is_empty() {
        Some("mcpServers")
    } else if config.get("servers").is_some_and(Value::is_object) {
        Some("servers")
    } else if config
        .get("mcp")
        .and_then(|mcp| mcp.get("servers"))
        .is_some_and(Value::is_object)
    {
        return config
            .get_mut("mcp")
            .and_then(|mcp| mcp.get_mut("servers"))
            .and_then(Value::as_object_mut)
            .expect("checked above");
    } else {
        None
    };
    match key {
        Some(key) => config
            .entry(key)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("an object, checked or just inserted"),
        None => config,
    }
}

/// Sets (`Some`) or removes (`None`) server `name` in `servers`.
fn apply(servers: &mut Map<String, Value>, name: &str, entry: Option<&Value>) -> Change {
    match entry {
        Some(entry) => match servers.get(name) {
            Some(current) if current == entry => Change::Unchanged,
            Some(_) => {
                servers.insert(name.to_owned(), entry.clone());
                Change::Updated
            }
            None => {
                servers.insert(name.to_owned(), entry.clone());
                Change::Added
            }
        },
        None => match servers.remove(name) {
            Some(_) => Change::Removed,
            None => Change::Absent,
        },
    }
}

/// Applies `edits` (`(name, Some(entry))` sets, `(name, None)` removes) to the servers map of an
/// `mcpServers`-style config. Returns what changed per server and the new text, `None` when
/// nothing changed. Fails, leaving the file alone, when the config can't be read.
pub fn update_servers(
    existing: Option<&str>,
    edits: &[(&str, Option<Value>)],
) -> Result<(Vec<Change>, Option<String>), String> {
    let mut config = parse_config(existing)?;
    let servers = servers_map(&mut config);
    let changes: Vec<Change> = edits
        .iter()
        .map(|(name, entry)| apply(servers, name, entry.as_ref()))
        .collect();
    if !changes.iter().any(|change| change.is_change()) {
        return Ok((changes, None));
    }
    Ok((changes, Some(render(config)?)))
}

/// The value after `flag` (`--root`) in server `name`'s `args`, if it has one.
pub fn server_arg(existing: &str, name: &str, flag: &str) -> Option<PathBuf> {
    let mut config = parse_config(Some(existing)).ok()?;
    let args = servers_map(&mut config).get(name)?.get("args")?.as_array()?;
    let position = args.iter().position(|arg| arg == flag)?;
    args.get(position + 1)?.as_str().map(PathBuf::from)
}

/// [`update_servers`] for Claude Code's `~/.claude.json`: local-scope servers of `project`
/// (`projects.<path>.mcpServers`), or user-scope ones (top-level `mcpServers`) without a project.
/// Every other key of the file is kept as it is.
pub fn update_claude_servers(
    existing: Option<&str>,
    project: Option<&Path>,
    edits: &[(&str, Option<Value>)],
) -> Result<(Vec<Change>, Option<String>), String> {
    let mut config = parse_config(existing)?;
    let scope = match project {
        Some(project) => {
            let projects = object_entry(&mut config, "projects")?;
            object_entry(projects, &project.to_string_lossy())?
        }
        None => &mut config,
    };
    let removing_only = edits.iter().all(|(_, entry)| entry.is_none());
    if removing_only && !scope.get("mcpServers").is_some_and(Value::is_object) {
        return Ok((vec![Change::Absent; edits.len()], None));
    }
    let servers = object_entry(scope, "mcpServers")?;
    let changes: Vec<Change> = edits
        .iter()
        .map(|(name, entry)| apply(servers, name, entry.as_ref()))
        .collect();
    if !changes.iter().any(|change| change.is_change()) {
        return Ok((changes, None));
    }
    Ok((changes, Some(render(config)?)))
}

/// The object at `key` in `map`, created when missing.
fn object_entry<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    map.entry(key)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| format!("`{key}` in the config isn't an object"))
}

/// Server `name`'s entry in Claude Code's config, for `project` or the user scope.
pub fn claude_server(existing: &str, project: Option<&Path>, name: &str) -> Option<Value> {
    let config = parse_config(Some(existing)).ok()?;
    let scope = match project {
        Some(project) => config
            .get("projects")?
            .get(project.to_string_lossy().as_ref())?
            .clone(),
        None => Value::Object(config),
    };
    scope.get("mcpServers")?.get(name).cloned()
}

#[cfg(test)]
#[path = "json_config_tests.rs"]
mod tests;
