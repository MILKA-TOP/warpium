use ::settings::{SupportedPlatforms, SyncToCloud};
use warp_core::define_settings_group;

/// Keymap context flag present while [`WarpiumSettings::sync_from_warp`] is on, so the command
/// palette offers the matching enable or disable action.
pub(crate) const SYNC_FROM_WARP_CONTEXT_FLAG: &str = "WarpiumSyncFromWarp";

/// Keymap context flag present while [`WarpiumSettings::upstream_check`] is on.
pub(crate) const UPSTREAM_CHECK_CONTEXT_FLAG: &str = "WarpiumUpstreamCheck";

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
]);
