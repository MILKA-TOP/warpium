//! What a button runs: built-in app actions, and the shell line for a `command` button.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

macro_rules! builtin_actions {
    ($($variant:ident => $id:literal),+ $(,)?) => {
        /// A built-in app action a button can trigger. Ids reuse the `warpctrl` local-control
        /// action catalog names (`crates/local_control/src/catalog.rs`) for the parameterless
        /// UI actions.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum BuiltinAction {
            $($variant,)+
        }

        impl BuiltinAction {
            pub const ALL: &[Self] = &[$(Self::$variant,)+];

            /// The catalog id, such as `surface.project_explorer.open`.
            pub fn id(self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)+
                }
            }

            pub fn from_id(id: &str) -> Option<Self> {
                match id {
                    $($id => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

builtin_actions! {
    WindowCreate => "window.create",
    TabCreate => "tab.create",
    SessionNext => "session.next",
    SessionPrevious => "session.previous",
    SessionReopenClosed => "session.reopen_closed",
    AppearanceFontSizeIncrease => "appearance.font_size.increase",
    AppearanceFontSizeDecrease => "appearance.font_size.decrease",
    AppearanceFontSizeReset => "appearance.font_size.reset",
    AppearanceZoomIncrease => "appearance.zoom.increase",
    AppearanceZoomDecrease => "appearance.zoom.decrease",
    AppearanceZoomReset => "appearance.zoom.reset",
    SurfaceSettingsOpen => "surface.settings.open",
    SurfaceCommandPaletteOpen => "surface.command_palette.open",
    SurfaceThemePickerOpen => "surface.theme_picker.open",
    SurfaceKeybindingsOpen => "surface.keybindings.open",
    SurfaceWarpDriveOpen => "surface.warp_drive.open",
    SurfaceWarpDriveToggle => "surface.warp_drive.toggle",
    SurfaceResourceCenterToggle => "surface.resource_center.toggle",
    SurfaceCodeReviewToggle => "surface.code_review.toggle",
    SurfaceProjectExplorerOpen => "surface.project_explorer.open",
    SurfaceGlobalSearchOpen => "surface.global_search.open",
    SurfaceConversationListOpen => "surface.conversation_list.open",
    SurfaceLeftPanelToggle => "surface.left_panel.toggle",
    SurfaceRightPanelToggle => "surface.right_panel.toggle",
    SurfaceVerticalTabsToggle => "surface.vertical_tabs.toggle",
}

/// The quoting and grouping rules of the shell a command runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFamily {
    /// bash, zsh and other POSIX shells.
    Posix,
    Fish,
    PowerShell,
}

/// Resolves a button's `cwd` (relative to the project root; the root itself when absent) to an
/// absolute directory, without touching the filesystem.
pub fn resolve_cwd(project_root: &Path, cwd: Option<&str>) -> PathBuf {
    let mut resolved = project_root.to_path_buf();
    let Some(cwd) = cwd else {
        return resolved;
    };
    for component in Path::new(cwd).components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    resolved
}

/// The line to type into an existing session so `command` runs in `cwd` with `env`, without
/// changing that session's own directory or (except in PowerShell) its environment. With no
/// `cwd` and no `env` the command is returned unchanged. The wrapper's closing part goes on its
/// own line, so a trailing `#` comment or newline in `command` can't swallow it.
pub fn shell_line(
    command: &str,
    cwd: Option<&Path>,
    env: &BTreeMap<String, String>,
    shell: ShellFamily,
) -> String {
    if cwd.is_none() && env.is_empty() {
        return command.to_owned();
    }
    match shell {
        ShellFamily::Posix => {
            let mut parts = Vec::new();
            if let Some(cwd) = cwd {
                parts.push(format!("cd -- {}", posix_quote(&cwd.to_string_lossy())));
            }
            for (name, value) in env {
                parts.push(format!("export {name}={}", posix_quote(value)));
            }
            parts.push(command.to_owned());
            format!("( {}\n)", parts.join(" && "))
        }
        ShellFamily::Fish => {
            let env = env
                .iter()
                .map(|(name, value)| format!("set -lx {name} {}; ", fish_quote(value)))
                .collect::<String>();
            match cwd {
                Some(cwd) => format!(
                    "if pushd {}; {env}{command}\npopd; end",
                    fish_quote(&cwd.to_string_lossy())
                ),
                None => format!("begin; {env}{command}\nend"),
            }
        }
        ShellFamily::PowerShell => {
            let env = env
                .iter()
                .map(|(name, value)| format!("$env:{name} = {}; ", powershell_quote(value)))
                .collect::<String>();
            match cwd {
                Some(cwd) => format!(
                    "& {{ Push-Location -LiteralPath {}; try {{ {env}{command}\n}} finally {{ Pop-Location }} }}",
                    powershell_quote(&cwd.to_string_lossy())
                ),
                None => format!("& {{ {env}{command}\n}}"),
            }
        }
    }
}

fn posix_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', r"\\").replace('\'', r"\'"))
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Whether `name` is a portable environment variable name.
pub(crate) fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

#[cfg(test)]
#[path = "action_tests.rs"]
mod tests;
