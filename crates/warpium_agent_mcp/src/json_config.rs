//! Registering the server with agents: an entry in an MCP JSON config (Warp's `.mcp.json`) and
//! the `claude mcp add` command for Claude Code.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use warpium_android::adb::shell_quote;

/// The server's name in MCP configs.
pub const SERVER_NAME: &str = "warpium-logcat";
/// The binary's file name, next to the app's executable in a bundle.
pub const BINARY_NAME: &str = "warpium-logcat";

/// The server's command-line arguments.
pub fn server_args(root: Option<&Path>) -> Vec<String> {
    let mut args = vec!["mcp".to_owned()];
    if let Some(root) = root {
        args.push("--root".to_owned());
        args.push(root.to_string_lossy().into_owned());
    }
    args
}

/// The config entry that starts the server over stdio.
pub fn server_entry(binary: &Path, root: Option<&Path>) -> Value {
    json!({
        "command": binary.to_string_lossy(),
        "args": server_args(root),
    })
}

/// The `claude mcp add` command that registers the server with Claude Code.
pub fn claude_mcp_add_command(binary: &Path, root: Option<&Path>) -> String {
    let mut parts = vec![
        "claude".to_owned(),
        "mcp".to_owned(),
        "add".to_owned(),
        SERVER_NAME.to_owned(),
        "--".to_owned(),
        shell_quote(&binary.to_string_lossy()),
    ];
    parts.extend(server_args(root).iter().map(|arg| shell_quote(arg)));
    parts.join(" ")
}

/// The JSON pointer of the servers map in `config`, in the wrapper forms Warp reads
/// (`mcpServers`, `servers`, `mcp.servers`); `None` for a bare map of servers.
fn servers_pointer(config: &Map<String, Value>) -> Option<&'static str> {
    if config.get("mcpServers").is_some_and(Value::is_object) {
        Some("/mcpServers")
    } else if config.get("servers").is_some_and(Value::is_object) {
        Some("/servers")
    } else if config
        .get("mcp")
        .and_then(|mcp| mcp.get("servers"))
        .is_some_and(Value::is_object)
    {
        Some("/mcp/servers")
    } else {
        None
    }
}

fn parse_config(existing: Option<&str>) -> Result<Map<String, Value>, String> {
    match existing.map(str::trim).filter(|text| !text.is_empty()) {
        None => Ok(Map::new()),
        Some(text) => match serde_json::from_str(text) {
            Ok(Value::Object(config)) => Ok(config),
            Ok(_) => Err("the MCP config isn't a JSON object".to_owned()),
            Err(err) => Err(format!("the MCP config isn't valid JSON: {err}")),
        },
    }
}

/// The config with this server's entry set to `entry`, or removed for `None`. Returns `None` when
/// nothing changes, and fails (leaving the file alone) when the config can't be read.
pub fn update_mcp_config(
    existing: Option<&str>,
    entry: Option<&Value>,
) -> Result<Option<String>, String> {
    let mut config = parse_config(existing)?;
    let pointer = servers_pointer(&config);
    let servers = match pointer {
        Some(pointer) => {
            let path: Vec<&str> = pointer.trim_start_matches('/').split('/').collect();
            let mut map = &mut config;
            for key in path {
                map = map
                    .get_mut(key)
                    .and_then(Value::as_object_mut)
                    .expect("servers_pointer only names objects");
            }
            map
        }
        // An empty file gets the common `mcpServers` wrapper; a bare map of servers stays bare.
        None if config.is_empty() => config
            .entry("mcpServers")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("just inserted an object"),
        None => &mut config,
    };
    let changed = match entry {
        Some(entry) => {
            if servers.get(SERVER_NAME) == Some(entry) {
                false
            } else {
                servers.insert(SERVER_NAME.to_owned(), entry.clone());
                true
            }
        }
        None => servers.remove(SERVER_NAME).is_some(),
    };
    if !changed {
        return Ok(None);
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(config))
        .map_err(|err| format!("couldn't write the MCP config: {err}"))?;
    text.push('\n');
    Ok(Some(text))
}

/// The `--root` of this server's entry in `existing`, if it has one.
pub fn registered_root(existing: &str) -> Option<PathBuf> {
    let config = parse_config(Some(existing)).ok()?;
    let servers = match servers_pointer(&config) {
        Some(pointer) => Value::Object(config.clone()).pointer(pointer)?.clone(),
        None => Value::Object(config),
    };
    let args = servers.get(SERVER_NAME)?.get("args")?.as_array()?.clone();
    let position = args.iter().position(|arg| arg == "--root")?;
    args.get(position + 1)?.as_str().map(PathBuf::from)
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
