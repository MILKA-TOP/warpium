use ::settings::{SupportedPlatforms, SyncToCloud};
use warp_core::define_settings_group;

/// Keymap context flag present while [`WarpiumSettings::sync_from_warp`] is on, so the command
/// palette offers the matching enable or disable action.
pub(crate) const SYNC_FROM_WARP_CONTEXT_FLAG: &str = "WarpiumSyncFromWarp";

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
]);
