//! One-way import and ongoing sync of user configuration from an official Warp installation
//! (Stable, Preview or Dev) into Warpium.
//!
//! This crate holds the platform-neutral, app-independent logic so it can be unit tested without
//! the `warp` app crate: locating Warp installations, the 3-way sync rule, file categories, the
//! key-level merge of `settings.toml`, the manifest of what was imported last time, backups and
//! atomic writes. The app wires it to the settings framework, the UI and the macOS user defaults.
//!
//! See `docs/warpium/warp-config-import.md` for the category table.

mod engine;
mod files;
mod fs_util;
mod key_values;
mod manifest;
mod report;
mod settings_toml;
mod source;
mod sync;

pub use engine::{
    CommitError, PreparedImport, commit_settings_merge, prepare_import, prepare_settings_merge,
};
pub use files::{FileCategory, FileGuard, file_categories, sync_file_categories};
pub use fs_util::{atomic_write, backup_existing};
pub use key_values::{KeyValuePlan, PRIVATE_SETTINGS_DENYLIST, plan_key_values};
pub use manifest::{MANIFEST_FILE_NAME, Manifest, ManifestLoad};
pub use report::ImportReport;
pub use settings_toml::{
    SETTINGS_DENYLIST, SettingsMerge, SettingsMergeError, merge_settings_toml, path_rewrites,
};
pub use source::{WarpChannel, WarpInstallation, discover_installations, most_recently_used};
pub use sync::{SyncDecision, content_hash, decide};

use std::path::PathBuf;

/// Name of the directory, inside Warpium's data directory, that holds per-run backups of Warpium
/// files the import overwrote.
pub const BACKUP_DIR_NAME: &str = ".import-backup";

/// Where Warpium keeps the configuration that the import writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarpiumTarget {
    /// Portable user data: themes, workflows, launch configurations, tab configs.
    pub data_dir: PathBuf,
    /// Home-relative config directory: skills and the global `.mcp.json`.
    pub home_config_dir: PathBuf,
    /// The GUI `settings.toml`.
    pub settings_file: PathBuf,
    /// The GUI `keybindings.yaml`.
    pub keybindings_file: PathBuf,
}

impl WarpiumTarget {
    /// Directory for the backups of one import run.
    pub fn backup_dir(&self, run_id: &str) -> PathBuf {
        self.data_dir.join(BACKUP_DIR_NAME).join(run_id)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.data_dir.join(MANIFEST_FILE_NAME)
    }
}

/// Options shared by every phase of one import run.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// Compute and report what would change without writing anything.
    pub dry_run: bool,
    /// Identifies this run; used as the backup directory name.
    pub run_id: String,
}
