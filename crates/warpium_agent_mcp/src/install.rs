//! Installing servers into the coding agents present on this machine, each in its own format and
//! in a scope that is never committed:
//!
//! - a project managed by arc (it has `.arc/`) gets them in arc's private layer,
//!   `<project>/.arc/private/mcp.json` (git-ignored), which arc delivers to every agent it runs.
//!   No agent's own config is touched then;
//! - otherwise Claude Code (local scope of the project, else user scope), Codex
//!   (`$CODEX_HOME/config.toml`) and Kimi Code (`$KIMI_CODE_HOME/mcp.json`), each when it is
//!   installed: its CLI is found or its config directory exists.
//!
//! Uninstalling removes the servers from every one of those configs that exists. Entries of other
//! servers are never touched, and installing again only rewrites what changed.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use command::blocking::Command;
use serde_json::{Value, json};

use crate::json_config::{claude_server, update_claude_servers, update_servers};
use crate::toml_config::{TomlServer, update_codex_servers};
use crate::{Change, McpServer};

/// Where agents and their configs are looked up.
#[derive(Clone, Debug)]
pub struct Environment {
    pub home: PathBuf,
    /// The project the servers are installed for, if one is open.
    pub project_root: Option<PathBuf>,
    /// `CODEX_HOME`, if set.
    pub codex_home: Option<PathBuf>,
    /// `KIMI_CODE_HOME`, if set.
    pub kimi_home: Option<PathBuf>,
    /// Directories searched for agent CLIs.
    pub search_path: Vec<PathBuf>,
}

impl Environment {
    /// The process environment. GUI apps on macOS get a minimal `PATH`, so the usual install
    /// locations of the CLIs are searched as well.
    pub fn from_process(home: PathBuf, project_root: Option<PathBuf>) -> Self {
        let from_env = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let mut search_path: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        search_path.extend([
            home.join(".local/bin"),
            home.join(".claude/local"),
            home.join(".npm-global/bin"),
            home.join(".bun/bin"),
            home.join(".cargo/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
        Self {
            codex_home: from_env("CODEX_HOME"),
            kimi_home: from_env("KIMI_CODE_HOME"),
            home,
            project_root,
            search_path,
        }
    }

    fn find_cli(&self, name: &str) -> Option<PathBuf> {
        self.search_path
            .iter()
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
    }

    fn arc_private_dir(&self) -> Option<PathBuf> {
        let root = self.project_root.as_ref()?;
        root.join(".arc")
            .is_dir()
            .then(|| root.join(".arc").join("private"))
    }

    fn claude_config(&self) -> PathBuf {
        self.home.join(".claude.json")
    }

    fn claude_installed(&self) -> bool {
        self.claude_config().is_file()
            || self.home.join(".claude").is_dir()
            || self.find_cli("claude").is_some()
    }

    fn codex_dir(&self) -> PathBuf {
        self.codex_home
            .clone()
            .unwrap_or_else(|| self.home.join(".codex"))
    }

    fn codex_installed(&self) -> bool {
        self.codex_dir().is_dir() || self.find_cli("codex").is_some()
    }

    /// Kimi Code's home: `KIMI_CODE_HOME`, `~/.kimi-code`, or the legacy kimi-cli `~/.kimi`
    /// when only that one exists.
    fn kimi_dir(&self) -> PathBuf {
        if let Some(home) = &self.kimi_home {
            return home.clone();
        }
        let current = self.home.join(".kimi-code");
        let legacy = self.home.join(".kimi");
        if !current.is_dir() && legacy.is_dir() {
            legacy
        } else {
            current
        }
    }

    fn kimi_installed(&self) -> bool {
        self.kimi_dir().is_dir() || self.find_cli("kimi").is_some()
    }

    /// `path` with the home directory shown as `~`.
    fn display(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }
}

/// What happened in one agent's config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The agent, such as "Claude Code".
    pub agent: &'static str,
    /// The config, with the home directory shown as `~`, and its scope.
    pub location: String,
    /// The change per server, in the order they were given, or why the config was left alone.
    pub result: Result<Vec<(String, Change)>, String>,
}

/// Installs `servers` into arc's private layer when the project uses arc, else into every agent
/// found. Empty when no agent is found.
pub fn install(env: &Environment, servers: &[McpServer]) -> Vec<Report> {
    if let Some(dir) = env.arc_private_dir() {
        return vec![json_target(
            env,
            "arc",
            &dir.join("mcp.json"),
            "private layer, run agents through arc to pick it up",
            servers,
            true,
            Mode::Install,
        )];
    }
    let mut reports = Vec::new();
    if env.claude_installed() {
        reports.push(claude_target(env, servers, Mode::Install));
    }
    if env.codex_installed() {
        reports.push(codex_target(env, servers, Mode::Install));
    }
    if env.kimi_installed() {
        reports.push(json_target(
            env,
            "Kimi Code",
            &env.kimi_dir().join("mcp.json"),
            "user",
            servers,
            false,
            Mode::Install,
        ));
    }
    reports
}

/// Removes `servers` from every config [`install`] may have written that exists. Configs without
/// any of them are left out of the result.
pub fn uninstall(env: &Environment, servers: &[McpServer]) -> Vec<Report> {
    let mut reports = Vec::new();
    if let Some(dir) = env.arc_private_dir() {
        let config = dir.join("mcp.json");
        if config.is_file() {
            reports.push(json_target(
                env,
                "arc",
                &config,
                "private layer",
                servers,
                true,
                Mode::Uninstall,
            ));
        }
    }
    if env.claude_config().is_file() {
        reports.push(claude_target(env, servers, Mode::Uninstall));
    }
    if env.codex_dir().join("config.toml").is_file() {
        reports.push(codex_target(env, servers, Mode::Uninstall));
    }
    let kimi = env.kimi_dir().join("mcp.json");
    if kimi.is_file() {
        reports.push(json_target(
            env,
            "Kimi Code",
            &kimi,
            "user",
            servers,
            false,
            Mode::Uninstall,
        ));
    }
    reports.retain(|report| match &report.result {
        Ok(changes) => changes.iter().any(|(_, change)| *change != Change::Absent),
        Err(_) => true,
    });
    reports
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Install,
    Uninstall,
}

fn names(servers: &[McpServer], changes: Vec<Change>) -> Vec<(String, Change)> {
    servers
        .iter()
        .map(|server| server.name.clone())
        .zip(changes)
        .collect()
}

/// An `mcpServers` JSON config (arc, Kimi Code).
fn json_target(
    env: &Environment,
    agent: &'static str,
    config: &Path,
    scope: &str,
    servers: &[McpServer],
    per_project: bool,
    mode: Mode,
) -> Report {
    let edits: Vec<(&str, Option<Value>)> = servers
        .iter()
        .map(|server| {
            let entry = (mode == Mode::Install).then(|| server.json_entry(per_project));
            (server.name.as_str(), entry)
        })
        .collect();
    let result = edit_file(config, |existing| update_servers(existing, &edits))
        .map(|changes| names(servers, changes));
    Report {
        agent,
        location: format!("{} ({scope})", env.display(config)),
        result,
    }
}

fn codex_target(env: &Environment, servers: &[McpServer], mode: Mode) -> Report {
    let config = env.codex_dir().join("config.toml");
    let commands: Vec<(String, Vec<String>)> = servers
        .iter()
        .map(|server| (server.command_str(), server.args_for(false)))
        .collect();
    let edits: Vec<(&str, Option<TomlServer<'_>>)> = servers
        .iter()
        .zip(&commands)
        .map(|(server, (command, args))| {
            let entry = (mode == Mode::Install).then_some(TomlServer { command, args });
            (server.name.as_str(), entry)
        })
        .collect();
    let result = edit_file(&config, |existing| update_codex_servers(existing, &edits))
        .map(|changes| names(servers, changes));
    Report {
        agent: "Codex",
        location: format!("{} (user)", env.display(&config)),
        result,
    }
}

/// Claude Code's entry for a server, as `claude mcp add` writes it.
fn claude_entry(server: &McpServer, per_project: bool) -> Value {
    json!({
        "type": "stdio",
        "command": server.command_str(),
        "args": server.args_for(per_project),
        "env": {},
    })
}

/// Claude Code: the project's local scope, else the user scope. Changes go through the `claude`
/// CLI when it is found, as Claude Code may rewrite its config while it runs; otherwise the
/// config is edited directly.
fn claude_target(env: &Environment, servers: &[McpServer], mode: Mode) -> Report {
    let config = env.claude_config();
    let project = env.project_root.as_deref();
    let scope = if project.is_some() { "local" } else { "user" };
    let location = match project {
        Some(project) => format!(
            "{} (local scope of {})",
            env.display(&config),
            env.display(project)
        ),
        None => format!("{} (user scope)", env.display(&config)),
    };
    let report = |result| Report {
        agent: "Claude Code",
        location: location.clone(),
        result,
    };
    let per_project = project.is_some();
    let edits: Vec<(&str, Option<Value>)> = servers
        .iter()
        .map(|server| {
            let entry = (mode == Mode::Install).then(|| claude_entry(server, per_project));
            (server.name.as_str(), entry)
        })
        .collect();
    let Some(cli) = env.find_cli("claude") else {
        let result = edit_file(&config, |existing| {
            update_claude_servers(existing, project, &edits)
        })
        .map(|changes| names(servers, changes));
        return report(result);
    };

    let existing = match read_optional(&config) {
        Ok(existing) => existing,
        Err(err) => return report(Err(err)),
    };
    let mut changes = Vec::with_capacity(servers.len());
    for (server, (_, entry)) in servers.iter().zip(&edits) {
        let current = existing
            .as_deref()
            .and_then(|text| claude_server(text, project, &server.name));
        let change = match (&current, entry) {
            (Some(current), Some(entry)) if current == entry => Change::Unchanged,
            (None, None) => Change::Absent,
            (Some(_), None) => Change::Removed,
            (Some(_), Some(_)) => Change::Updated,
            (None, Some(_)) => Change::Added,
        };
        if change.is_change() {
            if current.is_some()
                && let Err(err) = run_claude(
                    &cli,
                    project,
                    &["mcp", "remove", "--scope", scope, &server.name],
                )
            {
                return report(Err(err));
            }
            if entry.is_some() {
                let mut args = vec!["mcp", "add", "--scope", scope, &server.name, "--"];
                let command = server.command_str();
                let server_args = server.args_for(per_project);
                args.push(&command);
                args.extend(server_args.iter().map(String::as_str));
                if let Err(err) = run_claude(&cli, project, &args) {
                    return report(Err(err));
                }
            }
        }
        changes.push((server.name.clone(), change));
    }
    report(Ok(changes))
}

fn run_claude(cli: &Path, project: Option<&Path>, args: &[&str]) -> Result<(), String> {
    let mut command = Command::new(cli);
    command.args(args);
    if let Some(project) = project {
        command.current_dir(project);
    }
    let output = command
        .output()
        .map_err(|err| format!("couldn't run claude: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "`claude {}` failed: {}",
            args[..2].join(" "),
            stderr.trim()
        ))
    }
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("couldn't read {}: {err}", path.display())),
    }
}

/// Reads `path` (missing counts as empty), lets `update` compute the new text and writes it, if
/// any, through a temporary file renamed over the old one so readers never see half a file. The
/// old file's permissions are kept.
pub fn edit_file(
    path: &Path,
    update: impl FnOnce(Option<&str>) -> Result<(Vec<Change>, Option<String>), String>,
) -> Result<Vec<Change>, String> {
    let existing = read_optional(path)?;
    let (changes, text) = update(existing.as_deref())?;
    let Some(text) = text else {
        return Ok(changes);
    };
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".warpium-tmp");
        let temporary = PathBuf::from(temporary);
        std::fs::write(&temporary, text)?;
        if let Ok(metadata) = std::fs::metadata(path) {
            std::fs::set_permissions(&temporary, metadata.permissions())?;
        }
        std::fs::rename(&temporary, path)
    };
    write().map_err(|err| format!("couldn't write {}: {err}", path.display()))?;
    Ok(changes)
}

/// A short summary of `reports` for a notification.
pub fn summary(reports: &[Report], installing: bool) -> String {
    if reports.is_empty() {
        return if installing {
            "No coding agent found (Claude Code, Codex, Kimi Code or an arc project).".to_owned()
        } else {
            "Warpium's MCP servers weren't installed in any agent.".to_owned()
        };
    }
    let lines: Vec<String> = reports
        .iter()
        .map(|report| match &report.result {
            Ok(changes) => {
                let all_unchanged = changes.iter().all(|(_, change)| !change.is_change());
                let what = if all_unchanged {
                    if installing {
                        "already installed"
                    } else {
                        "nothing to remove"
                    }
                } else if installing {
                    "installed"
                } else {
                    "removed"
                };
                format!("{}: {what} in {}", report.agent, report.location)
            }
            Err(err) => format!("{}: failed, {err}", report.agent),
        })
        .collect();
    lines.join("\n")
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
