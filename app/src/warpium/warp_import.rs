//! Imports configuration from the official Warp app into Warpium and keeps it flowing.
//!
//! On every launch (while [`WarpiumSettings::sync_from_warp`] is on) and on demand from the
//! command palette, this syncs Warp's themes, workflows, launch configurations, tab configs,
//! skills, keybindings, `settings.toml` and user-defaults settings one way into Warpium. The
//! decisions live in the `warp_config_import` crate; this module runs them off the main thread and
//! applies the settings parts through the settings framework so they take effect immediately.

use std::collections::BTreeMap;
use std::time::Instant;

use ::settings::schema::SettingSchemaEntry;
use ::settings::{PublicPreferences, SettingsManager};
use warp_config_import::{
    CommitError, ImportOptions, ImportReport, PreparedImport, SettingsMerge, SettingsMergeError,
    WarpiumTarget, commit_settings_merge, prepare_import, prepare_settings_merge,
};
use warp_core::{safe_info, safe_warn};
use warpui::keymap::EditableBinding;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use super::sync_settings::{SYNC_FROM_WARP_CONTEXT_FLAG, WarpiumSettings};
use crate::settings_view::ToggleSettingActionPair;
use crate::util::bindings::BindingGroup;
use crate::view_components::DismissibleToast;
use crate::workspace::{ToastStack, WorkspaceAction};

/// Set to `1` to log what an import would do without writing anything.
const DRY_RUN_ENV: &str = "WARPIUM_IMPORT_DRY_RUN";

/// What started an import run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportTrigger {
    Startup,
    /// The "Import settings from Warp now" command; always reports its result.
    Manual,
}

/// Runs imports from Warp, at most one at a time.
#[derive(Default)]
pub(crate) struct WarpConfigImporter {
    running: bool,
}

impl Entity for WarpConfigImporter {
    type Event = ();
}

impl SingletonEntity for WarpConfigImporter {}

/// Registers the importer and starts the launch-time sync when it is enabled.
pub(crate) fn init(ctx: &mut AppContext) {
    let importer = ctx.add_singleton_model(|_| WarpConfigImporter::default());
    if *WarpiumSettings::as_ref(ctx).sync_from_warp {
        importer.update(ctx, |importer, ctx| {
            importer.run(ImportTrigger::Startup, ctx)
        });
    } else {
        log::info!("[Warp import] Sync from Warp is off; skipping the launch-time import");
    }
}

/// Registers the command palette actions for importing from Warp.
pub(crate) fn init_bindings(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_editable_bindings([EditableBinding::new(
        "workspace:import_settings_from_warp",
        "Import settings from Warp now",
        WorkspaceAction::ImportSettingsFromWarp,
    )
    .with_group(BindingGroup::Settings.as_str())
    .with_context_predicate(id!("Workspace"))]);

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "syncing settings from Warp on launch",
            WorkspaceAction::ToggleSyncFromWarp,
            &id!("Workspace"),
            SYNC_FROM_WARP_CONTEXT_FLAG,
        )],
        app,
    );
}

impl WarpConfigImporter {
    pub(crate) fn run(&mut self, trigger: ImportTrigger, ctx: &mut ModelContext<Self>) {
        if self.running {
            if trigger == ImportTrigger::Manual {
                show_toast(
                    DismissibleToast::default("An import from Warp is already running".to_owned()),
                    ctx,
                );
            }
            return;
        }
        let Some(home) = dirs::home_dir() else {
            log::warn!("[Warp import] Home directory unknown; skipping the import");
            return;
        };
        let Some(target) = warpium_target() else {
            log::warn!("[Warp import] Warpium's config directory is unknown; skipping the import");
            return;
        };
        let known_setting_paths = known_setting_paths();
        let options = ImportOptions {
            dry_run: std::env::var(DRY_RUN_ENV).is_ok_and(|value| value == "1"),
            run_id: chrono::Local::now().format("%Y%m%d-%H%M%S").to_string(),
        };

        self.running = true;
        let started = Instant::now();
        ctx.spawn(
            async move {
                let prepared = prepare_import(&home, &target, &known_setting_paths, &options);
                prepared.map(|prepared| RunContext {
                    prepared,
                    home,
                    target,
                    known_setting_paths,
                    options,
                })
            },
            move |importer, run, ctx| {
                importer.running = false;
                match run {
                    Some(run) => run.finish(trigger, started, ctx),
                    None => {
                        log::info!("[Warp import] No Warp installation found; nothing to import");
                        if trigger == ImportTrigger::Manual {
                            show_toast(
                                DismissibleToast::default(
                                    "No Warp installation found to import from".to_owned(),
                                ),
                                ctx,
                            );
                        }
                    }
                }
            },
        );
    }
}

/// Everything the main-thread half of a run needs.
struct RunContext {
    prepared: PreparedImport,
    home: std::path::PathBuf,
    target: WarpiumTarget,
    known_setting_paths: Vec<String>,
    options: ImportOptions,
}

impl RunContext {
    fn finish(
        self,
        trigger: ImportTrigger,
        started: Instant,
        ctx: &mut ModelContext<WarpConfigImporter>,
    ) {
        let RunContext {
            prepared,
            home,
            target,
            known_setting_paths,
            options,
        } = self;
        let PreparedImport {
            source,
            initial_import,
            mut manifest,
            mut report,
            settings,
        } = prepared;

        match settings {
            None => report.skipped.push((
                "settings.toml".to_owned(),
                "Warp has no settings.toml".to_owned(),
            )),
            Some(Err(err)) => report
                .skipped
                .push(("settings.toml".to_owned(), err.to_string())),
            Some(Ok(merge)) => {
                let retry = || {
                    prepare_settings_merge(
                        &source,
                        &target,
                        &home,
                        &known_setting_paths,
                        &manifest,
                        initial_import,
                    )
                };
                if let Some((base, merge_report)) =
                    apply_settings_merge(merge, retry, &target, &options, &mut report, ctx)
                {
                    manifest.settings = base;
                    report.absorb(merge_report);
                }
            }
        }

        #[cfg(target_os = "macos")]
        sync_private_settings(
            &source,
            initial_import,
            &options,
            &mut manifest.private_settings,
            &mut report,
            ctx,
        );

        if !options.dry_run
            && let Err(err) = manifest.save(&target.manifest_path())
        {
            log::error!("[Warp import] Failed to save the import manifest: {err}");
        }

        log_report(&report, initial_import, started);
        let notable = report.changed_count() > 0 || !report.errors.is_empty();
        if trigger == ImportTrigger::Manual || notable {
            let toast = if report.errors.is_empty() {
                DismissibleToast::success(report.summary())
            } else {
                DismissibleToast::error(report.summary())
            };
            show_toast(toast, ctx);
        }
    }
}

/// Writes the merged `settings.toml` and reloads settings from it. Returns the new manifest base
/// and the merge's report when the merge was applied (or, in a dry run, would be).
///
/// Runs on the main thread: the settings framework holds the file in memory and rewrites it
/// whole on every change, so writing it from elsewhere could be undone by a concurrent change.
fn apply_settings_merge(
    merge: SettingsMerge,
    retry: impl FnOnce() -> Option<Result<SettingsMerge, SettingsMergeError>>,
    target: &WarpiumTarget,
    options: &ImportOptions,
    report: &mut ImportReport,
    ctx: &mut ModelContext<WarpConfigImporter>,
) -> Option<(BTreeMap<String, String>, ImportReport)> {
    if !PublicPreferences::as_ref(ctx).is_settings_file() {
        report.skipped.push((
            "settings.toml".to_owned(),
            "the settings file is disabled in this build".to_owned(),
        ));
        return None;
    }

    let mut merge = merge;
    let mut committed = commit_settings_merge(&merge, target, options);
    if matches!(committed, Err(CommitError::TargetChanged)) {
        // Warpium changed its settings while the merge was computed; redo it on current contents.
        match retry() {
            Some(Ok(fresh)) => {
                merge = fresh;
                committed = commit_settings_merge(&merge, target, options);
            }
            Some(Err(err)) => {
                report
                    .skipped
                    .push(("settings.toml".to_owned(), err.to_string()));
                return None;
            }
            None => return None,
        }
    }

    match committed {
        Ok(written) => {
            if written {
                reload_settings_file(ctx);
            }
            Some((merge.base, merge.report))
        }
        Err(CommitError::TargetChanged) => {
            report.skipped.push((
                "settings.toml".to_owned(),
                "changed during the import; it will be synced next time".to_owned(),
            ));
            None
        }
        Err(CommitError::Io(err)) => {
            report
                .errors
                .push(("settings.toml".to_owned(), err.to_string()));
            None
        }
    }
}

/// Loads the just-written `settings.toml` into the in-memory settings, as the file watcher would,
/// so imported values apply at once and a later settings change can't write stale contents back.
fn reload_settings_file(ctx: &mut ModelContext<WarpConfigImporter>) {
    if let Err(err) = PublicPreferences::as_ref(ctx).reload_from_disk() {
        log::warn!("[Warp import] Reloading settings.toml after the import failed: {err}");
        return;
    }
    let failed_keys = SettingsManager::handle(ctx)
        .update(ctx, |manager, ctx| manager.reload_all_public_settings(ctx));
    if !failed_keys.is_empty() {
        log::warn!(
            "[Warp import] Imported settings Warpium can't use (defaults apply): {}",
            failed_keys.join(", ")
        );
    }
}

/// Syncs the settings Warp keeps in its user defaults domain rather than `settings.toml`.
#[cfg(target_os = "macos")]
fn sync_private_settings(
    source: &warp_config_import::WarpInstallation,
    initial_import: bool,
    options: &ImportOptions,
    base: &mut BTreeMap<String, warp_config_import::KeyValueBase>,
    report: &mut ImportReport,
    ctx: &mut ModelContext<WarpConfigImporter>,
) {
    use warp_config_import::{SyncDecision, plan_key_values};
    use warp_core::user_preferences::GetUserPreferences as _;
    use warpui_extras::user_preferences::UserPreferences as _;
    use warpui_extras::user_preferences::user_defaults::UserDefaultsPreferencesStorage;

    if !source.defaults_plist.is_file() {
        return;
    }
    let warp_defaults =
        UserDefaultsPreferencesStorage::new(Some(source.channel.bundle_id().to_owned()));

    let manager = SettingsManager::as_ref(ctx);
    let keys: Vec<String> = manager
        .all_storage_keys()
        .filter(|key| manager.is_private_for_storage_key(key))
        .cloned()
        .collect();
    let defaults: BTreeMap<String, String> = manager.default_values().collect();

    let read_all = |prefs: &dyn warpui_extras::user_preferences::UserPreferences| {
        keys.iter()
            .filter_map(|key| Some((key.clone(), prefs.read_value(key).ok()??)))
            .collect::<BTreeMap<_, _>>()
    };
    let source_values = read_all(&warp_defaults);
    let target_values = read_all(ctx.private_user_preferences());

    for plan in plan_key_values(&keys, &source_values, &target_values, base, initial_import) {
        let new_value = match plan.decision {
            SyncDecision::Create | SyncDecision::Update | SyncDecision::Overwrite => {
                plan.source_value.clone()
            }
            SyncDecision::Remove => defaults.get(&plan.key).cloned(),
            SyncDecision::InSync | SyncDecision::KeepLocal | SyncDecision::Conflict => None,
        };
        let applied = match new_value {
            Some(_) if options.dry_run => Ok(None),
            Some(value) => SettingsManager::handle(ctx)
                .update(ctx, |manager, ctx| {
                    manager.update_setting_with_storage_key(&plan.key, value, false, ctx)
                })
                .map(|()| {
                    ctx.private_user_preferences()
                        .read_value(&plan.key)
                        .ok()
                        .flatten()
                })
                .map_err(|err| format!("{err:#}")),
            None => Ok(None),
        };
        plan.finish(applied, base, report);
    }
}

/// Warpium's locations for the imported configuration.
fn warpium_target() -> Option<WarpiumTarget> {
    Some(WarpiumTarget {
        data_dir: warp_core::paths::data_dir(),
        home_config_dir: warp_core::paths::warp_home_config_dir()?,
        settings_file: crate::settings::user_preferences_toml_file_path(),
        keybindings_file: crate::keyboard::keybinding_file_path(),
    })
}

/// Dotted `settings.toml` paths of every public setting Warpium registers.
fn known_setting_paths() -> Vec<String> {
    inventory::iter::<SettingSchemaEntry>
        .into_iter()
        .filter(|entry| !entry.is_private)
        .map(|entry| match entry.hierarchy {
            Some(hierarchy) => format!("{hierarchy}.{}", entry.storage_key),
            None => entry.storage_key.to_owned(),
        })
        .collect()
}

fn log_report(report: &ImportReport, initial_import: bool, started: Instant) {
    let elapsed_ms = started.elapsed().as_millis();
    log::info!(
        "[Warp import] {} (initial={initial_import}, unchanged={}, took {elapsed_ms} ms)",
        report.summary(),
        report.unchanged
    );
    if !report.conflicts.is_empty() {
        safe_info!(
            safe: ("[Warp import] {} items changed in both apps; kept Warpium's", report.conflicts.len()),
            full: ("[Warp import] Changed in both apps, kept Warpium's: {}", report.conflicts.join(", "))
        );
    }
    for (item, reason) in &report.skipped {
        log::debug!("[Warp import] Skipped {item}: {reason}");
    }
    for (item, err) in &report.errors {
        safe_warn!(
            safe: ("[Warp import] Failed to import an item"),
            full: ("[Warp import] Failed to import {item}: {err}")
        );
    }
    log::debug!(
        "[Warp import] imported={:?} updated={:?} reset={:?}",
        report.imported,
        report.updated,
        report.removed
    );
}

fn show_toast(
    toast: DismissibleToast<WorkspaceAction>,
    ctx: &mut ModelContext<WarpConfigImporter>,
) {
    let window_id = ctx
        .windows()
        .active_window()
        .or_else(|| ctx.windows().ordered_window_ids().first().copied());
    let Some(window_id) = window_id else {
        return;
    };
    ToastStack::handle(ctx).update(ctx, |stack, ctx| {
        stack.add_ephemeral_toast(toast, window_id, ctx);
    });
}
