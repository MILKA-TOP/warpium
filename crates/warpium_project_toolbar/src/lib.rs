//! Warpium: a per-project toolbar of command buttons, JetBrains-style.
//!
//! A project's team config lives in a committed `.warpium/toolbar.toml`; personal overrides live
//! in `~/.warp-oss/projects/<project-id>/toolbar.toml` and are merged by button `id`. This crate
//! holds the config format, the override merge, project detection, the trust bookkeeping for
//! committed commands and the mapping of button actions to what the app runs. The app supplies
//! the UI, the file watcher, persistence and the execution itself.

mod action;
mod config;
mod icons;
mod project;
mod run_configs;
mod template;
mod trust;

pub use action::{BuiltinAction, ShellFamily, resolve_cwd, shell_line};
pub use config::{
    ButtonAction, CommandAction, CommandTarget, ConfigError, ConfigSource, ParsedConfig,
    ToolbarButton, merge, parse_config,
};
pub use icons::{BUNDLED_ICONS, ButtonIcon, bundled_icon_path, parse_icon};
pub use project::{
    CONFIG_FILE_NAME, PROJECT_CONFIG_DIR, ProjectContext, ProjectSource, detect_project,
    personal_config_path, project_id, team_config_path,
};
pub use run_configs::{
    InlineFit, RunConfigItem, RunConfigKind, fit_inline_buttons, run_config_items,
    selected_run_config, toolbar_buttons,
};
pub use template::{PERSONAL_TEMPLATE, TEAM_TEMPLATE};
pub use trust::{ToolbarUserState, TrustListing, content_hash, needs_trust, trust_listings};
