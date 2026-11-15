//! Warpium: its MCP servers for coding agents, `warpium-logcat` (device logs and crashes) and
//! `warpium-code-index` (Kotlin/Java declarations), bundled next to the app's executable.
//!
//! - While `warpium.logcat_mcp` is on, the Logcat server is registered in Warp's global MCP config
//!   (`~/.warp-oss/.mcp.json`), which Warp starts for its agent; turning the setting off removes
//!   the entry.
//! - "Install MCP servers for AI agents" writes both servers into the other agents found on this
//!   machine (arc's private layer for arc projects, else Claude Code, Codex and Kimi Code), and
//!   "Uninstall" removes them again. See `warpium_agent_mcp::install`.

use std::path::{Path, PathBuf};

use warpium_agent_mcp::McpServer;
use warpium_agent_mcp::install::{self, Environment};
use warpium_agent_mcp::json_config::{server_arg, update_servers};
use warpui::keymap::EditableBinding;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use super::sync_settings::{
    LOGCAT_MCP_CONTEXT_FLAG, WarpiumAgentSettings, WarpiumAgentSettingsChangedEvent,
};
use super::{is_warpium_channel, show_toast};
use crate::settings_view::ToggleSettingActionPair;
use crate::util::bindings::BindingGroup;
use crate::view_components::DismissibleToast;
use crate::warp_managed_paths_watcher::warp_home_mcp_config_file_path;
use crate::workspace::WorkspaceAction;

const LOGCAT_SERVER: &str = "warpium-logcat";
const CODE_INDEX_SERVER: &str = "warpium-code-index";

/// Keeps Warp's MCP config in line with the setting, and installs the servers into other agents
/// on request.
pub(crate) struct AgentMcpModel {
    /// The project the servers work on (stack frames, the code index). Without one, Warp's entry
    /// keeps the project it was last registered with, and other agents get no `--root`.
    project_root: Option<PathBuf>,
    /// An install or uninstall is running.
    busy: bool,
}

impl Entity for AgentMcpModel {
    type Event = ();
}

impl SingletonEntity for AgentMcpModel {}

pub(crate) fn init(ctx: &mut AppContext) {
    ctx.add_singleton_model(|ctx: &mut ModelContext<AgentMcpModel>| {
        ctx.subscribe_to_model(
            &WarpiumAgentSettings::handle(ctx),
            |model, _, event, ctx| match event {
                WarpiumAgentSettingsChangedEvent::LogcatMcp { .. } => {
                    model.sync_warp_registration(true, ctx)
                }
            },
        );
        let model = AgentMcpModel {
            project_root: None,
            busy: false,
        };
        // Points an existing entry at this build's binary, or removes one left from when the
        // setting was on.
        model.sync_warp_registration(false, ctx);
        model
    });
}

/// Registers the setting's palette toggle and the install commands.
pub(crate) fn init_bindings(ctx: &mut AppContext) {
    use warpui::keymap::macros::*;

    ctx.register_editable_bindings([
        EditableBinding::new(
            "workspace:install_agent_mcp_servers",
            "Install MCP servers for AI agents (Logcat, code index)",
            WorkspaceAction::InstallAgentMcpServers,
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(id!("Workspace")),
        EditableBinding::new(
            "workspace:uninstall_agent_mcp_servers",
            "Uninstall MCP servers for AI agents (Logcat, code index)",
            WorkspaceAction::UninstallAgentMcpServers,
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(id!("Workspace")),
    ]);

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "Logcat MCP server for Warp's agent",
            WorkspaceAction::ToggleLogcatMcp,
            &id!("Workspace"),
            LOGCAT_MCP_CONTEXT_FLAG,
        )],
        ctx,
    );
}

/// A binary bundled next to the app's executable, if this build ships it.
fn bundled_binary(name: &str) -> Option<PathBuf> {
    let binary = std::env::current_exe().ok()?.parent()?.join(name);
    binary.is_file().then_some(binary)
}

/// The server `name` started as `<binary> mcp [--root <root>]`, if its binary is bundled.
fn server(name: &str, root: Option<&Path>) -> Option<McpServer> {
    Some(McpServer {
        name: name.to_owned(),
        command: bundled_binary(name)?,
        args: vec!["mcp".to_owned()],
        project_args: root
            .map(|root| vec!["--root".to_owned(), root.to_string_lossy().into_owned()])
            .unwrap_or_default(),
    })
}

fn missing_binaries_toast() -> DismissibleToast<WorkspaceAction> {
    DismissibleToast::error(format!(
        "This build doesn't include {LOGCAT_SERVER} or {CODE_INDEX_SERVER}. Build them with \
         `cargo build --release -p warpium_logcat_mcp -p code_index` and register them by hand \
         (see WARPIUM.md)."
    ))
}

impl AgentMcpModel {
    /// Uses `root` from now on. A `None` root keeps the current one.
    pub(crate) fn set_project_root(&mut self, root: Option<PathBuf>, ctx: &mut ModelContext<Self>) {
        if root.is_none() || root == self.project_root {
            return;
        }
        self.project_root = root;
        self.sync_warp_registration(false, ctx);
    }

    /// Installs (`install`) or uninstalls both servers in the coding agents found, then shows
    /// where.
    pub(crate) fn install_into_agents(&mut self, install: bool, ctx: &mut ModelContext<Self>) {
        if self.busy {
            return;
        }
        let Some(home) = dirs::home_dir() else {
            show_toast(
                DismissibleToast::error("Couldn't find the home directory.".to_owned()),
                ctx,
            );
            return;
        };
        let root = self.project_root.clone();
        let servers: Vec<McpServer> = if install {
            [LOGCAT_SERVER, CODE_INDEX_SERVER]
                .into_iter()
                .filter_map(|name| server(name, root.as_deref()))
                .collect()
        } else {
            // Removal only needs the names.
            [LOGCAT_SERVER, CODE_INDEX_SERVER]
                .into_iter()
                .map(|name| McpServer {
                    name: name.to_owned(),
                    command: PathBuf::new(),
                    args: Vec::new(),
                    project_args: Vec::new(),
                })
                .collect()
        };
        if servers.is_empty() {
            show_toast(missing_binaries_toast(), ctx);
            return;
        }
        self.busy = true;
        let env = Environment::from_process(home, root);
        ctx.spawn(
            blocking::unblock(move || {
                let reports = if install {
                    install::install(&env, &servers)
                } else {
                    install::uninstall(&env, &servers)
                };
                let names: Vec<String> = servers.iter().map(|server| server.name.clone()).collect();
                (reports, names)
            }),
            move |model, (reports, names), ctx| {
                model.busy = false;
                for report in &reports {
                    if let Err(err) = &report.result {
                        log::warn!("[Agent MCP] {} at {}: {err}", report.agent, report.location);
                    }
                }
                let failed = reports.iter().any(|report| report.result.is_err());
                let summary = install::summary(&reports, install);
                let message = if install && !reports.is_empty() {
                    format!("{}:\n{summary}", names.join(", "))
                } else {
                    summary
                };
                let toast = if failed || (install && reports.is_empty()) {
                    DismissibleToast::error(message)
                } else {
                    DismissibleToast::success(message)
                };
                show_toast(toast, ctx);
            },
        );
    }

    /// Writes or removes the Logcat server's entry in Warp's MCP config. `user_toggled` reports
    /// problems the user should see.
    fn sync_warp_registration(&self, user_toggled: bool, ctx: &mut ModelContext<Self>) {
        if !is_warpium_channel() {
            return;
        }
        let Some(config_path) = warp_home_mcp_config_file_path() else {
            return;
        };
        let enabled = *WarpiumAgentSettings::as_ref(ctx).logcat_mcp;
        let registration = if enabled {
            let Some(binary) = bundled_binary(LOGCAT_SERVER) else {
                if user_toggled {
                    show_toast(missing_binaries_toast(), ctx);
                }
                return;
            };
            Some((binary, self.project_root.clone()))
        } else {
            None
        };
        ctx.spawn(
            blocking::unblock(move || write_warp_registration(&config_path, registration)),
            move |_, result, ctx| {
                if let Err(message) = result {
                    log::warn!("[Agent MCP] Couldn't update Warp's MCP config: {message}");
                    if user_toggled {
                        show_toast(
                            DismissibleToast::error(format!(
                                "Couldn't update Warp's MCP servers: {message}"
                            )),
                            ctx,
                        );
                    }
                }
            },
        );
    }
}

/// Sets (`Some((binary, root))`) or removes (`None`) the Logcat server's entry in the MCP config
/// at `config_path`. Without a root, an existing entry's root is kept.
fn write_warp_registration(
    config_path: &Path,
    registration: Option<(PathBuf, Option<PathBuf>)>,
) -> Result<(), String> {
    install::edit_file(config_path, |existing| {
        let entry = registration.map(|(binary, root)| {
            let root = root
                .or_else(|| existing.and_then(|text| server_arg(text, LOGCAT_SERVER, "--root")));
            McpServer {
                name: LOGCAT_SERVER.to_owned(),
                command: binary,
                args: vec!["mcp".to_owned()],
                project_args: root
                    .map(|root| vec!["--root".to_owned(), root.to_string_lossy().into_owned()])
                    .unwrap_or_default(),
            }
            .json_entry(true)
        });
        update_servers(existing, &[(LOGCAT_SERVER, entry)])
    })
    .map(|_| ())
}
