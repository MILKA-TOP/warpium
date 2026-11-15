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
]);
