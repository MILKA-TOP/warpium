use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A channel of the official Warp app whose configuration can be imported.
///
/// The directory names and bundle ids mirror `warp_core::paths` and `app/Cargo.toml` for those
/// channels on macOS; they are persisted on disk by Warp and never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WarpChannel {
    Stable,
    Preview,
    Dev,
}

impl WarpChannel {
    pub const ALL: [WarpChannel; 3] = [WarpChannel::Stable, WarpChannel::Preview, WarpChannel::Dev];

    pub fn display_name(self) -> &'static str {
        match self {
            WarpChannel::Stable => "Warp",
            WarpChannel::Preview => "Warp Preview",
            WarpChannel::Dev => "Warp Dev",
        }
    }

    /// The bundle id, which is also the `NSUserDefaults` domain of the channel.
    pub fn bundle_id(self) -> &'static str {
        match self {
            WarpChannel::Stable => "dev.warp.Warp-Stable",
            WarpChannel::Preview => "dev.warp.Warp-Preview",
            WarpChannel::Dev => "dev.warp.Warp-Dev",
        }
    }

    /// `data_dir()` / `config_local_dir()` on macOS: themes, workflows, `settings.toml`,
    /// `keybindings.yaml`.
    fn data_dir_name(self) -> &'static str {
        match self {
            WarpChannel::Stable => ".warp",
            WarpChannel::Preview => ".warp-preview",
            WarpChannel::Dev => ".warp-dev",
        }
    }

    /// `warp_home_config_dir()`: skills and `.mcp.json`. Preview still shares Stable's.
    fn home_config_dir_name(self) -> &'static str {
        match self {
            WarpChannel::Stable | WarpChannel::Preview => ".warp",
            WarpChannel::Dev => ".warp-dev",
        }
    }

    pub fn manifest_name(self) -> &'static str {
        match self {
            WarpChannel::Stable => "stable",
            WarpChannel::Preview => "preview",
            WarpChannel::Dev => "dev",
        }
    }
}

/// A Warp installation found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarpInstallation {
    pub channel: WarpChannel,
    pub data_dir: PathBuf,
    pub home_config_dir: PathBuf,
    /// The user defaults property list of the channel.
    pub defaults_plist: PathBuf,
    /// When this installation last wrote its settings, if known.
    pub last_used: Option<SystemTime>,
}

impl WarpInstallation {
    pub fn for_channel(channel: WarpChannel, home: &Path) -> Self {
        let data_dir = home.join(channel.data_dir_name());
        let defaults_plist = home
            .join("Library/Preferences")
            .join(format!("{}.plist", channel.bundle_id()));
        let last_used = [
            defaults_plist.clone(),
            data_dir.join("settings.toml"),
            data_dir.clone(),
        ]
        .iter()
        .filter_map(|path| path.metadata().and_then(|meta| meta.modified()).ok())
        .max();
        Self {
            channel,
            home_config_dir: home.join(channel.home_config_dir_name()),
            data_dir,
            defaults_plist,
            last_used,
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.data_dir.join("settings.toml")
    }

    fn exists(&self) -> bool {
        self.data_dir.is_dir() || self.defaults_plist.is_file()
    }
}

/// Returns every Warp installation with data under `home`.
pub fn discover_installations(home: &Path) -> Vec<WarpInstallation> {
    WarpChannel::ALL
        .into_iter()
        .map(|channel| WarpInstallation::for_channel(channel, home))
        .filter(WarpInstallation::exists)
        .collect()
}

/// Picks the installation used most recently. Ties, and installations without a known time, are
/// resolved in favour of Stable, then Preview, then Dev.
pub fn most_recently_used(installations: Vec<WarpInstallation>) -> Option<WarpInstallation> {
    // `max_by_key` keeps the last maximum; iterate in reverse so the earlier channel wins ties.
    installations
        .into_iter()
        .rev()
        .max_by_key(|installation| installation.last_used)
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
