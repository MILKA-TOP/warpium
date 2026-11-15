//! Warpium: the Logcat MCP server for coding agents (`warpium-logcat`, bundled next to the app's
//! executable). While `warpium.logcat_mcp` is on, it is registered in Warp's global MCP config
//! (`~/.warp-oss/.mcp.json`), which Warp starts for its agent; turning the setting off removes
//! the entry. Claude Code gets a copyable `claude mcp add` command instead.

use std::path::{Path, PathBuf};

use warpium_logcat_mcp::registry::{
    BINARY_NAME, claude_mcp_add_command, registered_root, server_entry, update_mcp_config,
};
use warpui::clipboard::ClipboardContent;
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

/// Keeps the MCP config entry in line with the setting and the current project.
pub(crate) struct LogcatMcpRegistration {
    /// The Android project the registered server resolves stack frames in. Without one, the
    /// entry keeps the project it was last registered with.
    project_root: Option<PathBuf>,
}

impl Entity for LogcatMcpRegistration {
    type Event = ();
}

impl SingletonEntity for LogcatMcpRegistration {}

pub(crate) fn init(ctx: &mut AppContext) {
    ctx.add_singleton_model(|ctx: &mut ModelContext<LogcatMcpRegistration>| {
        ctx.subscribe_to_model(
            &WarpiumAgentSettings::handle(ctx),
            |registration, _, event, ctx| match event {
                WarpiumAgentSettingsChangedEvent::LogcatMcp { .. } => registration.sync(true, ctx),
            },
        );
        let registration = LogcatMcpRegistration { project_root: None };
        // Points an existing entry at this build's binary, or removes one left from when the
        // setting was on.
        registration.sync(false, ctx);
        registration
    });
}

/// Registers the setting's palette toggle and the Claude Code command.
pub(crate) fn init_bindings(ctx: &mut AppContext) {
    use warpui::keymap::macros::*;

    ctx.register_editable_bindings([EditableBinding::new(
        "workspace:copy_logcat_mcp_config",
        "Copy Logcat MCP config for Claude Code",
        WorkspaceAction::CopyLogcatMcpConfig,
    )
    .with_group(BindingGroup::Navigation.as_str())
    .with_context_predicate(id!("Workspace"))]);

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "Logcat MCP server",
            WorkspaceAction::ToggleLogcatMcp,
            &id!("Workspace"),
            LOGCAT_MCP_CONTEXT_FLAG,
        )],
        ctx,
    );
}

/// The `warpium-logcat` binary next to the app's executable, if this build ships it.
fn bundled_binary() -> Option<PathBuf> {
    let binary = std::env::current_exe().ok()?.parent()?.join(BINARY_NAME);
    binary.is_file().then_some(binary)
}

impl LogcatMcpRegistration {
    /// Registers the server for `root` from now on. A `None` root keeps the current one.
    pub(crate) fn set_project_root(&mut self, root: Option<PathBuf>, ctx: &mut ModelContext<Self>) {
        if root.is_none() || root == self.project_root {
            return;
        }
        self.project_root = root;
        self.sync(false, ctx);
    }

    /// Copies the `claude mcp add` command for `root` (else the current project) to the
    /// clipboard.
    pub(crate) fn copy_claude_command(&mut self, root: Option<PathBuf>, ctx: &mut ModelContext<Self>) {
        let Some(binary) = bundled_binary() else {
            show_toast(missing_binary_toast(), ctx);
            return;
        };
        let root = root.or_else(|| self.project_root.clone());
        let command = claude_mcp_add_command(&binary, root.as_deref());
        ctx.clipboard().write(ClipboardContent::plain_text(command));
        let message = if root.is_some() {
            "Copied the claude mcp add command for this project. Run it in the project's directory."
        } else {
            "Copied the claude mcp add command. Open an Android project first to have stack \
             frames resolved to its files."
        };
        show_toast(DismissibleToast::success(message.to_owned()), ctx);
    }

    /// Writes or removes the config entry. `user_toggled` reports problems the user should see.
    fn sync(&self, user_toggled: bool, ctx: &mut ModelContext<Self>) {
        if !is_warpium_channel() {
            return;
        }
        let Some(config_path) = warp_home_mcp_config_file_path() else {
            return;
        };
        let enabled = *WarpiumAgentSettings::as_ref(ctx).logcat_mcp;
        let registration = if enabled {
            let Some(binary) = bundled_binary() else {
                if user_toggled {
                    show_toast(missing_binary_toast(), ctx);
                }
                return;
            };
            Some((binary, self.project_root.clone()))
        } else {
            None
        };
        ctx.spawn(
            blocking::unblock(move || write_registration(&config_path, registration)),
            move |_, result, ctx| {
                if let Err(message) = result {
                    log::warn!("[Logcat MCP] Couldn't update Warp's MCP config: {message}");
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

fn missing_binary_toast() -> DismissibleToast<WorkspaceAction> {
    DismissibleToast::error(format!(
        "This build doesn't include {BINARY_NAME}. Build it with `cargo build --release -p \
         warpium_logcat_mcp` and register it by hand (see WARPIUM.md)."
    ))
}

/// Sets (`Some((binary, root))`) or removes (`None`) the server's entry in the MCP config at
/// `config_path`. Without a root, an existing entry's root is kept.
fn write_registration(
    config_path: &Path,
    registration: Option<(PathBuf, Option<PathBuf>)>,
) -> Result<(), String> {
    let existing = match std::fs::read_to_string(config_path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err.to_string()),
    };
    let entry = registration.map(|(binary, root)| {
        let root = root.or_else(|| existing.as_deref().and_then(registered_root));
        server_entry(&binary, root.as_deref())
    });
    let Some(text) = update_mcp_config(existing.as_deref(), entry.as_ref())? else {
        return Ok(());
    };
    if let Some(dir) = config_path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    // Written aside and renamed, so Warp's watcher never reads a half-written file.
    let temporary = config_path.with_extension("json.warpium-tmp");
    std::fs::write(&temporary, text).map_err(|err| err.to_string())?;
    std::fs::rename(&temporary, config_path).map_err(|err| err.to_string())
}
