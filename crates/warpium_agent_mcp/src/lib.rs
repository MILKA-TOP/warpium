//! Registering Warpium's MCP servers (`warpium-logcat`, `warpium-code-index`) with coding agents,
//! each in its own config format: JSON `mcpServers` maps (Warp, Kimi Code, arc), Codex's TOML and
//! Claude Code's CLI. Entries of other servers are never touched.

pub mod install;
pub mod json_config;
pub mod toml_config;

use std::path::PathBuf;

use serde_json::{Value, json};

/// A server started over stdio.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    /// Arguments naming the project (`--root <dir>`), only used in per-project configs. Shared
    /// configs leave them out, so the server works on the directory the agent runs in.
    pub project_args: Vec<String>,
}

impl McpServer {
    pub fn command_str(&self) -> String {
        self.command.to_string_lossy().into_owned()
    }

    /// The arguments for a per-project (`per_project`) or a shared config.
    pub fn args_for(&self, per_project: bool) -> Vec<String> {
        let mut args = self.args.clone();
        if per_project {
            args.extend(self.project_args.iter().cloned());
        }
        args
    }

    /// The `{"command", "args"}` entry of JSON configs.
    pub fn json_entry(&self, per_project: bool) -> Value {
        json!({
            "command": self.command_str(),
            "args": self.args_for(per_project),
        })
    }
}

/// What an install or uninstall did to one server in one config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Updated,
    Unchanged,
    Removed,
    /// Nothing to remove.
    Absent,
}
