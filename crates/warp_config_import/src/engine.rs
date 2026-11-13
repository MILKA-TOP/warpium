use std::io;
use std::path::Path;

use crate::files::{file_categories, sync_file_categories};
use crate::fs_util::{atomic_write, backup_existing, file_mode};
use crate::manifest::{Manifest, ManifestLoad};
use crate::report::ImportReport;
use crate::settings_toml::{SettingsMerge, SettingsMergeError, merge_settings_toml, path_rewrites};
use crate::source::{WarpInstallation, discover_installations, most_recently_used};
use crate::sync::content_hash;
use crate::{ImportOptions, WarpiumTarget};

/// The state of an import run after the work that needs no app state: the file categories are
/// synced and the settings merge is computed but not yet written.
#[derive(Debug)]
pub struct PreparedImport {
    pub source: WarpInstallation,
    pub initial_import: bool,
    pub manifest: Manifest,
    pub report: ImportReport,
    /// `None` when Warp has no `settings.toml`.
    pub settings: Option<Result<SettingsMerge, SettingsMergeError>>,
}

/// Picks the most recently used Warp installation under `home`, syncs its files into `target`
/// and computes the merge of its `settings.toml`. Returns `None` if Warp isn't installed.
///
/// Blocking file IO: run it off the main thread.
pub fn prepare_import(
    home: &Path,
    target: &WarpiumTarget,
    known_setting_paths: &[String],
    options: &ImportOptions,
) -> Option<PreparedImport> {
    let installations = discover_installations(home);
    let found = installations
        .iter()
        .map(|installation| installation.channel.manifest_name())
        .collect::<Vec<_>>()
        .join(", ");
    let source = most_recently_used(installations)?;
    log::info!(
        "[Warp import] Importing from {} (most recently used of: {found})",
        source.channel.display_name()
    );

    let load = Manifest::load(&target.manifest_path());
    if let ManifestLoad::Unreadable(err) = &load {
        log::warn!("[Warp import] Ignoring unreadable import manifest: {err}");
    }
    let initial_import = load.is_initial_import();
    let mut manifest = load.into_manifest();
    if let Some(previous) = &manifest.source_channel
        && previous != source.channel.manifest_name()
    {
        log::info!(
            "[Warp import] Source changed from {previous} to {}",
            source.channel.manifest_name()
        );
    }
    manifest.source_channel = Some(source.channel.manifest_name().to_owned());

    let mut report = ImportReport {
        source: Some(source.channel.display_name().to_owned()),
        dry_run: options.dry_run,
        ..Default::default()
    };
    sync_file_categories(
        &file_categories(&source, target),
        target,
        &mut manifest,
        initial_import,
        options,
        &mut report,
    );

    let settings = prepare_settings_merge(
        &source,
        target,
        home,
        known_setting_paths,
        &manifest,
        initial_import,
    );
    Some(PreparedImport {
        source,
        initial_import,
        manifest,
        report,
        settings,
    })
}

/// Reads both `settings.toml` files and merges them. `None` when Warp has no settings file.
pub fn prepare_settings_merge(
    source: &WarpInstallation,
    target: &WarpiumTarget,
    home: &Path,
    known_setting_paths: &[String],
    manifest: &Manifest,
    initial_import: bool,
) -> Option<Result<SettingsMerge, SettingsMergeError>> {
    let source_settings = match std::fs::read_to_string(source.settings_file()) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
        Err(err) => return Some(Err(SettingsMergeError::SourceUnparseable(err.to_string()))),
    };
    let target_settings = match std::fs::read_to_string(&target.settings_file) {
        Ok(contents) => Some(contents),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Some(Err(SettingsMergeError::TargetUnparseable(err.to_string()))),
    };
    Some(merge_settings_toml(
        &source_settings,
        target_settings.as_deref(),
        known_setting_paths,
        &path_rewrites(source, target, home),
        &manifest.settings,
        initial_import,
    ))
}

/// Why a settings merge was not written.
#[derive(Debug)]
pub enum CommitError {
    /// Warpium's `settings.toml` changed after the merge was computed; compute it again.
    TargetChanged,
    Io(io::Error),
}

/// Writes a computed settings merge to Warpium's `settings.toml`, backing up the previous file.
/// Returns whether the file was written.
///
/// The settings framework keeps the file's contents in memory and writes it whole, so call this on
/// the thread that owns the settings and reload them right after a write.
pub fn commit_settings_merge(
    merge: &SettingsMerge,
    target: &WarpiumTarget,
    options: &ImportOptions,
) -> Result<bool, CommitError> {
    let current = match std::fs::read(&target.settings_file) {
        Ok(bytes) => Some(content_hash(&bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(CommitError::Io(err)),
    };
    if current != merge.target_snapshot {
        return Err(CommitError::TargetChanged);
    }
    let Some(merged) = &merge.merged else {
        return Ok(false);
    };
    if options.dry_run {
        return Ok(false);
    }
    backup_existing(
        &target.settings_file,
        &target.backup_dir(&options.run_id),
        Path::new("settings.toml"),
    )
    .map_err(CommitError::Io)?;
    let mode = file_mode(&target.settings_file).or(Some(0o644));
    atomic_write(&target.settings_file, merged.as_bytes(), mode).map_err(CommitError::Io)?;
    Ok(true)
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
