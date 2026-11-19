use ::settings::{SupportedPlatforms, SyncToCloud};
use warp_core::define_settings_group;

/// Keymap context flag present while [`WarpiumSettings::sync_from_warp`] is on, so the command
/// palette offers the matching enable or disable action.
pub(crate) const SYNC_FROM_WARP_CONTEXT_FLAG: &str = "WarpiumSyncFromWarp";

/// Keymap context flag present while [`WarpiumSettings::upstream_check`] is on.
pub(crate) const UPSTREAM_CHECK_CONTEXT_FLAG: &str = "WarpiumUpstreamCheck";

/// Keymap context flag present while [`WarpiumSettings::update_check`] is on.
pub(crate) const UPDATE_CHECK_CONTEXT_FLAG: &str = "WarpiumUpdateCheck";

/// Keymap context flag present while [`WarpiumSettings::project_toolbar`] is on.
pub(crate) const PROJECT_TOOLBAR_CONTEXT_FLAG: &str = "WarpiumProjectToolbar";

/// Keymap context flag present while [`WarpiumSettings::code_index`] is on.
pub(crate) const CODE_INDEX_CONTEXT_FLAG: &str = "WarpiumCodeIndex";

/// Keymap context flag present while [`WarpiumSettings::android_tools`] is on.
pub(crate) const ANDROID_CONTEXT_FLAG: &str = "WarpiumAndroid";

define_settings_group!(WarpiumSettings, settings: [
    sync_from_warp: SyncFromWarp {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::MAC,
        sync_to_cloud: SyncToCloud::Never,
        surface: ::settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "warpium.sync_from_warp",
        description: "Whether Warpium imports configuration from the official Warp app on every launch.",
    },
    upstream_check: UpstreamCheck {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: ::settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "warpium.upstream_check",
        description: "Whether Warpium checks upstream Warp (warpdotdev/warp) for new commits and shows a notification when the fork is behind.",
    },
    upstream_check_interval_hours: UpstreamCheckIntervalHours {
        type: u64,
        default: 6,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: ::settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "warpium.upstream_check_interval_hours",
        description: "Hours between automatic checks of upstream Warp for new commits (1 to 168).",
    },
    upstream_check_min_commits: UpstreamCheckMinCommits {
        type: u64,
        default: 1,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: ::settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "warpium.upstream_check_min_commits",
        description: "How many new upstream Warp commits it takes to show the notification.",
    },
    update_check: UpdateCheck {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::MAC,
        sync_to_cloud: SyncToCloud::Never,
        surface: ::settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "warpium.update_check",
        description: "Whether Warpium checks GitHub for new Warpium releases and offers to install them. Official Warp updates are never offered.",
    },
    update_check_interval_hours: UpdateCheckIntervalHours {
        type: u64,
        default: 6,
        supported_platforms: SupportedPlatforms::MAC,
        sync_to_cloud: SyncToCloud::Never,
        surface: ::settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "warpium.update_check_interval_hours",
        description: "Hours between automatic checks for new Warpium releases (1 to 168).",
    },
    project_toolbar: ProjectToolbar {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: ::settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "warpium.project_toolbar",
        description: "Whether Warpium shows the project toolbar (buttons from .warpium/toolbar.toml and personal overrides) in the top-right corner of the window.",
    },
    code_index: CodeIndex {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: ::settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "warpium.code_index",
        description: "Whether Warpium indexes Kotlin and Java declarations of the open project for Go to Class and Go to Symbol. The index lives in the user cache directory, never in the project.",
    },
    android_tools: AndroidTools {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: ::settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "warpium.android",
        description: "Whether Warpium offers its Android tools: the device picker, running apps on an emulator or device, and the Logcat pane. adb is only contacted once one of them is used.",
    },
]);

/// Keymap context flag present while [`WarpiumAgentSettings::logcat_mcp`] is on.
pub(crate) const LOGCAT_MCP_CONTEXT_FLAG: &str = "WarpiumLogcatMcp";

pub(crate) use agent::{LogcatMcp, WarpiumAgentSettings, WarpiumAgentSettingsChangedEvent};

// Kept apart from `WarpiumSettings` so the many subscribers matching on its events don't need an
// arm for every agent setting. In its own module because each `define_settings_group!` imports
// `ChangeEventReason`, so two groups can't share one.
mod agent {
    use ::settings::{SupportedPlatforms, SyncToCloud};
    use warp_core::define_settings_group;

    define_settings_group!(WarpiumAgentSettings, settings: [
        logcat_mcp: LogcatMcp {
            type: bool,
            default: false,
            supported_platforms: SupportedPlatforms::ALL,
            sync_to_cloud: SyncToCloud::Never,
            surface: ::settings::SettingSurfaces::GUI,
            private: false,
            toml_path: "warpium.logcat_mcp",
            description: "Whether Warpium registers its Logcat MCP server in Warp's MCP servers, so agents can read the project's Android app logs and crashes. The server keeps logs in memory only, scoped to one app unless an agent asks for all.",
        },
    ]);
}

/// Keymap context flag present while [`WarpiumBottomPanelSettings::bottom_panel`] is on.
pub(crate) const BOTTOM_PANEL_CONTEXT_FLAG: &str = "WarpiumBottomPanel";

pub(crate) use bottom_panel::{BottomPanel, WarpiumBottomPanelSettings};

// Its own group for the same reason as `agent` above.
mod bottom_panel {
    use ::settings::{SupportedPlatforms, SyncToCloud};
    use warp_core::define_settings_group;

    define_settings_group!(WarpiumBottomPanelSettings, settings: [
        bottom_panel: BottomPanel {
            type: bool,
            default: true,
            supported_platforms: SupportedPlatforms::ALL,
            sync_to_cloud: SyncToCloud::Never,
            surface: ::settings::SettingSurfaces::GUI,
            private: false,
            toml_path: "warpium.bottom_panel",
            description: "Whether Warpium offers the bottom panel: a per-tab dock below the split layout where terminals and Logcat keep running out of the way, collapsible to a strip of status chips (cmd-j).",
        },
    ]);
}

/// Keymap context flag present while [`WarpiumContainerPasteSettings::container_image_paste`] is
/// on.
pub(crate) const CONTAINER_IMAGE_PASTE_CONTEXT_FLAG: &str = "WarpiumContainerImagePaste";

pub(crate) use container_paste::{ContainerImagePaste, WarpiumContainerPasteSettings};

// Its own group for the same reason as `agent` above.
mod container_paste {
    use ::settings::{SupportedPlatforms, SyncToCloud};
    use warp_core::define_settings_group;

    define_settings_group!(WarpiumContainerPasteSettings, settings: [
        container_image_paste: ContainerImagePaste {
            type: bool,
            default: true,
            supported_platforms: SupportedPlatforms::ALL,
            sync_to_cloud: SyncToCloud::Never,
            surface: ::settings::SettingSurfaces::GUI,
            private: false,
            toml_path: "warpium.container_image_paste",
            description: "Whether pasting or dropping an image into a terminal running a Docker/Podman container or an SSH session copies the image there and pastes its path, so CLI agents inside (Claude Code, Codex, ...) can read it.",
        },
        container_image_paste_container: ContainerImagePasteContainer {
            type: String,
            default: String::new(),
            supported_platforms: SupportedPlatforms::ALL,
            sync_to_cloud: SyncToCloud::Never,
            surface: ::settings::SettingSurfaces::GUI,
            private: false,
            toml_path: "warpium.container_image_paste_container",
            description: "The container (name or id; prefix it with `podman:` or `nerdctl:` for those engines) to paste images into while the terminal runs a command that isn't recognized as a container session, such as a wrapper script. Empty: detected containers only.",
        },
    ]);
}
