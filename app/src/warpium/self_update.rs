//! Offers and installs new Warpium releases published on GitHub (`MILKA-TOP/warpium`).
//!
//! On the OSS channel, while [`WarpiumSettings::update_check`] is on, this checks the latest
//! release on launch and then every `warpium.update_check_interval_hours`, and the workspace shows
//! a banner when a newer one exists. "Update now" downloads the release's disk image, checks it
//! against the release's SHA-256, swaps the new app bundle in and relaunches. Official Warp builds
//! are never fetched: Warp's own updater stays off for the OSS channel. The protocol, decisions
//! and install steps live in the `warpium_self_update` crate; this module supplies the HTTP
//! client, the schedule, persistence and the UI hooks.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::{AsyncWriteExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use warp_core::user_preferences::GetUserPreferences as _;
use warp_core::{safe_info, safe_warn};
use warp_errors::report_error;
use warpium_self_update::{
    CheckOutcome, CheckState, ChecksumError, HttpRequest, HttpResponse, InstallError, Installed,
    Release, Sha256Hasher, Trigger, WarpiumVersion,
};
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::keymap::EditableBinding;
use warpui::platform::TerminationMode;
use warpui::{AppContext, Entity, ModelContext, ModelSpawner, SingletonEntity};
use warpui_extras::user_preferences::UserPreferences as _;

use super::show_toast;
use super::sync_settings::{
    UPDATE_CHECK_CONTEXT_FLAG, WarpiumSettings, WarpiumSettingsChangedEvent,
};
use crate::channel::{Channel, ChannelState};
use crate::settings_view::ToggleSettingActionPair;
use crate::util::bindings::BindingGroup;
use crate::view_components::{DismissibleToast, ToastLink};
use crate::workspace::WorkspaceAction;

/// The release this build was made from, set by CI (see `script/warpium/version`).
const EMBEDDED_VERSION: Option<&str> = option_env!("WARPIUM_VERSION_TAG");
/// Overrides [`EMBEDDED_VERSION`] at runtime, so a build can pretend to be older and be offered
/// the latest release.
const VERSION_OVERRIDE_ENV: &str = "WARPIUM_UPDATE_CURRENT_VERSION_OVERRIDE";
/// Private-preferences key holding the serialized [`CheckState`].
const STATE_KEY: &str = "WarpiumSelfUpdateState";
/// Private-preferences key holding the [`InstallRecord`] of an update awaiting its first launch.
const INSTALL_RECORD_KEY: &str = "WarpiumSelfUpdateInstall";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const USER_AGENT: &str = "Warpium";

/// The installed bundle to open once this process exits, set when the user restarts for an update.
/// Kept outside the model because termination, which reads it, can run while the model is being
/// updated.
static RELAUNCH_BUNDLE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// An installed update whose previous bundle is kept until the new version has launched.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstallRecord {
    tag: String,
    backup: PathBuf,
}

#[derive(Debug, Clone)]
enum Stage {
    Idle,
    Downloading {
        release: Release,
        percent: Option<u8>,
    },
    Installing {
        release: Release,
    },
    /// Installed over the running app; a restart switches to it.
    Installed {
        release: Release,
        bundle: PathBuf,
    },
    Failed {
        release: Release,
        message: String,
        /// The verified disk image to install by hand, when the app can't update where it is.
        dmg: Option<PathBuf>,
    },
}

/// What the workspace banner shows about Warpium updates.
pub(crate) struct UpdateBanner {
    pub(crate) heading: String,
    pub(crate) description: String,
    pub(crate) kind: UpdateBannerKind,
}

pub(crate) enum UpdateBannerKind {
    /// A newer release is available: Update now, Release notes, Skip this version.
    Available,
    /// Downloading or installing; no actions.
    InProgress,
    /// Installed; Restart now.
    Installed,
    /// Failed; Download manually, and Reveal DMG when the disk image is kept.
    Failed { can_reveal_dmg: bool },
}

/// Checks for, offers and installs Warpium releases.
pub(crate) struct SelfUpdater {
    /// The running build's version; `None` for builds without a release tag, which never offer
    /// updates.
    current: Option<WarpiumVersion>,
    /// Remembered across launches; also holds the latest release.
    state: CheckState,
    stage: Stage,
    checking: bool,
    /// Whether this launch runs scheduled checks.
    polling: bool,
    next_check: Option<SpawnedFutureHandle>,
    /// The offered release whose banner the user closed for this session.
    dismissed_tag: Option<String>,
    /// Whether a manual check found the offered release, so it is shown even while scheduled
    /// checks are off.
    manually_found: bool,
    /// Whether the user closed the progress, installed or failure banner.
    status_hidden: bool,
    client: Arc<http_client::Client>,
}

impl Entity for SelfUpdater {
    type Event = ();
}

impl SingletonEntity for SelfUpdater {}

/// Registers the updater and, for `poll` launches, starts the scheduled checks. The updater is
/// registered in every launch mode because the command palette action is.
pub(crate) fn init(poll: bool, ctx: &mut AppContext) {
    if is_supported_channel() {
        // Warpium must never offer or install official Warp builds.
        debug_assert!(
            ChannelState::releases_base_url().is_empty()
                && !ChannelState::show_autoupdate_menu_items(),
            "Warp's own autoupdate must stay disabled for the OSS channel"
        );
    }
    let updater = ctx.add_singleton_model(|ctx: &mut ModelContext<SelfUpdater>| {
        ctx.subscribe_to_model(
            &WarpiumSettings::handle(ctx),
            |updater, _, event, ctx| match event {
                WarpiumSettingsChangedEvent::UpdateCheck { .. }
                | WarpiumSettingsChangedEvent::UpdateCheckIntervalHours { .. } => {
                    updater.reschedule(ctx);
                    ctx.notify();
                }
                WarpiumSettingsChangedEvent::SyncFromWarp { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheck { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheckIntervalHours { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheckMinCommits { .. } => {}
            },
        );
        SelfUpdater::new(poll, ctx)
    });
    if poll {
        updater.update(ctx, |updater, ctx| {
            updater.finish_previous_install(ctx);
            updater.reschedule(ctx);
        });
    }
}

/// Registers the command palette actions for Warpium updates.
pub(crate) fn init_bindings(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_editable_bindings([EditableBinding::new(
        "workspace:check_for_warpium_updates",
        "Check for Warpium updates",
        WorkspaceAction::CheckForWarpiumUpdates,
    )
    .with_group(BindingGroup::Settings.as_str())
    .with_context_predicate(id!("Workspace"))]);

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "Warpium update checks",
            WorkspaceAction::ToggleWarpiumUpdateCheck,
            &id!("Workspace"),
            UPDATE_CHECK_CONTEXT_FLAG,
        )],
        app,
    );
}

/// Starts the installed version once this process has exited, if an update asked for it. Called
/// while the app terminates.
pub(crate) fn spawn_relaunch_if_requested() {
    let Some(bundle) = relaunch_bundle().take() else {
        return;
    };
    // Like Warp's updater: a shell waits for this process to exit, so the dock doesn't show two
    // icons, then opens a new instance of the (now replaced) bundle.
    let result = command::blocking::Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"while ps -p "$1" >/dev/null 2>&1; do sleep 0.2; done; /usr/bin/open -n "$2""#)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(&bundle)
        .spawn();
    match result {
        Ok(_) => log::info!("[Warpium update] Relaunching into the installed version"),
        Err(err) => log::error!("[Warpium update] Couldn't relaunch after updating: {err}"),
    }
}

/// Forgets a requested relaunch because the user cancelled quitting. The "installed" banner
/// stays, so the user can restart later.
pub(crate) fn cancel_relaunch() {
    relaunch_bundle().take();
}

fn relaunch_bundle() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    RELAUNCH_BUNDLE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl SelfUpdater {
    fn new(polling: bool, ctx: &mut ModelContext<Self>) -> Self {
        let runtime_override = std::env::var(VERSION_OVERRIDE_ENV).ok();
        let current = warpium_self_update::resolve_current_version(
            runtime_override.as_deref(),
            EMBEDDED_VERSION,
        );
        if is_supported_channel() {
            match &current {
                Some(current) => {
                    log::info!("[Warpium update] Running Warpium {current}");
                }
                None => log::info!(
                    "[Warpium update] This build has no release version; update checks are off"
                ),
            }
        }
        let state = ctx
            .private_user_preferences()
            .read_value(STATE_KEY)
            .ok()
            .flatten()
            .map(|json| CheckState::from_json(&json))
            .unwrap_or_default();
        Self {
            current,
            state,
            stage: Stage::Idle,
            checking: false,
            polling,
            next_check: None,
            dismissed_tag: None,
            manually_found: false,
            status_hidden: false,
            client: Arc::new(http_client::Client::new_external()),
        }
    }

    /// The banner to show, if any.
    pub(crate) fn banner(&self, ctx: &AppContext) -> Option<UpdateBanner> {
        if !is_supported_channel() {
            return None;
        }
        let in_progress = |heading: String| UpdateBanner {
            heading,
            description: "Keep working; Warpium restarts when it's done.".to_owned(),
            kind: UpdateBannerKind::InProgress,
        };
        match &self.stage {
            Stage::Idle => {
                let current = self.current.as_ref()?;
                let release = self.offered(ctx)?;
                Some(UpdateBanner {
                    heading: warpium_self_update::banner_heading(current, release),
                    description: warpium_self_update::banner_description().to_owned(),
                    kind: UpdateBannerKind::Available,
                })
            }
            _ if self.status_hidden => None,
            Stage::Downloading { release, percent } => Some(in_progress(match percent {
                Some(percent) => format!("Downloading Warpium {}… {percent}%", release.tag),
                None => format!("Downloading Warpium {}…", release.tag),
            })),
            Stage::Installing { release } => {
                Some(in_progress(format!("Installing Warpium {}…", release.tag)))
            }
            Stage::Installed { release, .. } => Some(UpdateBanner {
                heading: format!("Warpium {} is installed.", release.tag),
                description: "Restart Warpium to start using it.".to_owned(),
                kind: UpdateBannerKind::Installed,
            }),
            Stage::Failed {
                release,
                message,
                dmg,
            } => Some(UpdateBanner {
                heading: format!("Couldn't update to Warpium {}.", release.tag),
                description: format!(
                    "{}. Your current version is unchanged.",
                    capitalize(message)
                ),
                kind: UpdateBannerKind::Failed {
                    can_reveal_dmg: dmg.is_some(),
                },
            }),
        }
    }

    /// The newer release to offer, unless the user skipped it or closed its banner.
    fn offered(&self, ctx: &AppContext) -> Option<&Release> {
        if !(*WarpiumSettings::as_ref(ctx).update_check || self.manually_found) {
            return None;
        }
        warpium_self_update::update_to_offer(
            self.current.as_ref(),
            self.state.latest.as_ref(),
            self.state.skipped_tag.as_deref(),
        )
        .filter(|release| self.dismissed_tag.as_deref() != Some(release.tag.as_str()))
    }

    /// The release the banner is about.
    fn banner_release(&self, ctx: &AppContext) -> Option<&Release> {
        match &self.stage {
            Stage::Idle => self.offered(ctx),
            Stage::Downloading { release, .. }
            | Stage::Installing { release }
            | Stage::Installed { release, .. }
            | Stage::Failed { release, .. } => Some(release),
        }
    }

    pub(crate) fn open_release_notes(&self, ctx: &mut ModelContext<Self>) {
        if let Some(url) = self.banner_release(ctx).map(Release::page_url) {
            ctx.open_url(&url);
        }
    }

    /// Opens the release page, where the disk image can be downloaded by hand.
    pub(crate) fn download_manually(&self, ctx: &mut ModelContext<Self>) {
        self.open_release_notes(ctx);
    }

    /// Never offers the offered release again; a newer one is still offered.
    pub(crate) fn skip_offered(&mut self, ctx: &mut ModelContext<Self>) {
        let Some(tag) = self.offered(ctx).map(|release| release.tag.clone()) else {
            return;
        };
        log::info!("[Warpium update] Skipping Warpium {tag}");
        self.state.skipped_tag = Some(tag);
        self.manually_found = false;
        self.save_state(ctx);
        ctx.notify();
    }

    /// Hides the offer banner until the next launch.
    pub(crate) fn dismiss_offer(&mut self, ctx: &mut ModelContext<Self>) {
        self.dismissed_tag = self.offered(ctx).map(|release| release.tag.clone());
        ctx.notify();
    }

    /// Hides the progress, installed or failure banner. Closing a failure also hides the offer
    /// until the next launch.
    pub(crate) fn dismiss_status(&mut self, ctx: &mut ModelContext<Self>) {
        match &self.stage {
            Stage::Failed { release, .. } => {
                self.dismissed_tag = Some(release.tag.clone());
                self.set_stage(Stage::Idle, ctx);
            }
            Stage::Downloading { .. } | Stage::Installing { .. } | Stage::Installed { .. } => {
                self.status_hidden = true;
                ctx.notify();
            }
            Stage::Idle => {}
        }
    }

    /// Shows the kept disk image in Finder.
    pub(crate) fn reveal_dmg(&self) {
        let Stage::Failed { dmg: Some(dmg), .. } = &self.stage else {
            return;
        };
        if let Err(err) = command::blocking::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(dmg)
            .spawn()
        {
            log::warn!("[Warpium update] Couldn't reveal the disk image: {err}");
        }
    }

    /// Quits and starts the installed version.
    pub(crate) fn restart(&self, ctx: &mut ModelContext<Self>) {
        let Stage::Installed { bundle, .. } = &self.stage else {
            return;
        };
        *relaunch_bundle() = Some(bundle.clone());
        ctx.terminate_app(TerminationMode::Cancellable, None);
    }

    /// Checks now and reports the result in a toast, whatever it is.
    pub(crate) fn check_now(&mut self, ctx: &mut ModelContext<Self>) {
        let message = if !is_supported_channel() {
            Some("Warpium updates are only available in Warpium builds".to_owned())
        } else if self.current.is_none() {
            Some(
                "This is a development build without a release version, so it doesn't check for \
                 Warpium updates"
                    .to_owned(),
            )
        } else if self.checking {
            Some("A check for Warpium updates is already running".to_owned())
        } else {
            match &self.stage {
                Stage::Idle | Stage::Failed { .. } => None,
                Stage::Downloading { release, .. } | Stage::Installing { release } => {
                    Some(format!("Warpium {} is being installed", release.tag))
                }
                Stage::Installed { release, .. } => Some(format!(
                    "Warpium {} is installed; restart Warpium to start using it",
                    release.tag
                )),
            }
        };
        match message {
            Some(message) => show_toast(DismissibleToast::default(message), ctx),
            None => self.run_check(Trigger::Manual, ctx),
        }
    }

    /// Downloads, verifies and installs the offered release, then relaunches.
    pub(crate) fn start_update(&mut self, ctx: &mut ModelContext<Self>) {
        if !matches!(self.stage, Stage::Idle | Stage::Failed { .. }) {
            return;
        }
        let release = match &self.stage {
            Stage::Failed { release, .. } => Some(release.clone()),
            _ => self.offered(ctx).cloned(),
        };
        let Some(release) = release else {
            return;
        };
        let Some(version) = release.version() else {
            return;
        };
        let bundle = match running_bundle_path() {
            Ok(bundle) => bundle,
            Err(message) => {
                self.set_stage(
                    Stage::Failed {
                        release,
                        message,
                        dmg: None,
                    },
                    ctx,
                );
                return;
            }
        };

        log::info!("[Warpium update] Updating to Warpium {}", release.tag);
        self.set_stage(
            Stage::Downloading {
                release: release.clone(),
                percent: Some(0),
            },
            ctx,
        );
        let client = self.client.clone();
        let spawner = ctx.spawner();
        let work_dir = update_dir().join(&release.tag);
        ctx.spawn(
            async move {
                let dmg = download_release(&client, &release, &work_dir, &spawner)
                    .await
                    .map_err(UpdateFailure::Download)?;
                let installing = release.clone();
                let _ = spawner
                    .spawn(move |updater, ctx| {
                        updater.set_stage(
                            Stage::Installing {
                                release: installing,
                            },
                            ctx,
                        )
                    })
                    .await;
                let mountpoint = work_dir.join(format!("mnt-{}", std::process::id()));
                install_off_main_thread(dmg.clone(), version, bundle.clone(), mountpoint)
                    .await
                    .map(|installed| (installed, bundle))
                    .map_err(|err| UpdateFailure::Install { err, dmg })
            },
            |updater, result, ctx| updater.finish_update(result, ctx),
        );
    }

    fn finish_update(
        &mut self,
        result: Result<(Installed, PathBuf), UpdateFailure>,
        ctx: &mut ModelContext<Self>,
    ) {
        let release = match &self.stage {
            Stage::Downloading { release, .. } | Stage::Installing { release } => release.clone(),
            Stage::Idle | Stage::Installed { .. } | Stage::Failed { .. } => return,
        };
        match result {
            Ok((installed, bundle)) => {
                safe_info!(
                    safe: ("[Warpium update] Installed Warpium {}", release.tag),
                    full: ("[Warpium update] Installed Warpium {} at {}; previous version kept at {}", release.tag, bundle.display(), installed.backup.display())
                );
                let record = InstallRecord {
                    tag: release.tag.clone(),
                    backup: installed.backup,
                };
                self.save_install_record(Some(&record), ctx);
                remove_in_background(update_dir());
                self.set_stage(Stage::Installed { release, bundle }, ctx);
                self.restart(ctx);
            }
            Err(failure) => {
                let dmg = failure.dmg_to_reveal();
                let message = failure.to_string();
                failure.log();
                self.set_stage(
                    Stage::Failed {
                        release,
                        message,
                        dmg,
                    },
                    ctx,
                );
            }
        }
    }

    fn set_download_progress(&mut self, percent: Option<u8>, ctx: &mut ModelContext<Self>) {
        if let Stage::Downloading {
            percent: current, ..
        } = &mut self.stage
        {
            *current = percent;
            ctx.notify();
        }
    }

    fn set_stage(&mut self, stage: Stage, ctx: &mut ModelContext<Self>) {
        self.stage = stage;
        self.status_hidden = false;
        ctx.notify();
    }

    /// After the first launch of an installed update, deletes the previous version's backup.
    fn finish_previous_install(&mut self, ctx: &mut ModelContext<Self>) {
        remove_in_background(update_dir());
        let Some(record) = ctx
            .private_user_preferences()
            .read_value(INSTALL_RECORD_KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<InstallRecord>(&json).ok())
        else {
            return;
        };
        let launched_update = self
            .current
            .as_ref()
            .is_some_and(|current| current.tag() == record.tag);
        if !launched_update {
            safe_warn!(
                safe: ("[Warpium update] Warpium {} was installed but this launch is another version; keeping the backup", record.tag),
                full: ("[Warpium update] Warpium {} was installed but this launch is another version; keeping the backup at {}", record.tag, record.backup.display())
            );
            return;
        }
        log::info!(
            "[Warpium update] Updated to Warpium {}; removing the previous version",
            record.tag
        );
        let backup = record.backup;
        std::thread::spawn(move || {
            if let Err(err) = warpium_self_update::remove_backup(&backup) {
                safe_warn!(
                    safe: ("[Warpium update] Couldn't remove the previous version: {err}"),
                    full: ("[Warpium update] Couldn't remove the previous version at {}: {err}", backup.display())
                );
            }
        });
        self.save_install_record(None, ctx);
    }

    fn is_enabled(&self, ctx: &AppContext) -> bool {
        is_supported_channel()
            && self.current.is_some()
            && *WarpiumSettings::as_ref(ctx).update_check
    }

    /// Replaces any pending scheduled check with one at the next due time, or none when checks
    /// are off.
    fn reschedule(&mut self, ctx: &mut ModelContext<Self>) {
        if let Some(handle) = self.next_check.take() {
            handle.abort();
        }
        if !self.polling || !self.is_enabled(ctx) {
            return;
        }
        let delay =
            warpium_self_update::next_check_delay(&self.state, unix_now(), self.interval(ctx));
        self.next_check = Some(ctx.spawn(
            async move {
                Timer::after(delay).await;
            },
            |updater, _, ctx| {
                updater.next_check = None;
                if updater.checking {
                    return;
                }
                updater.run_check(Trigger::Scheduled, ctx);
            },
        ));
    }

    fn interval(&self, ctx: &AppContext) -> Duration {
        warpium_self_update::check_interval(
            *WarpiumSettings::as_ref(ctx).update_check_interval_hours,
        )
    }

    fn run_check(&mut self, trigger: Trigger, ctx: &mut ModelContext<Self>) {
        self.checking = true;
        let state = self.state.clone();
        let client = self.client.clone();
        let max_backoff = self.interval(ctx);
        ctx.spawn(
            async move {
                let now = unix_now();
                let (state, outcome) =
                    warpium_self_update::run_check(state, now, max_backoff, trigger, |request| {
                        fetch(client.clone(), request)
                    })
                    .await;
                (state, outcome, now)
            },
            move |updater, (state, outcome, now), ctx| {
                updater.finish_check(trigger, state, outcome, now, ctx);
            },
        );
    }

    fn finish_check(
        &mut self,
        trigger: Trigger,
        state: CheckState,
        outcome: CheckOutcome,
        now: u64,
        ctx: &mut ModelContext<Self>,
    ) {
        self.checking = false;
        // A skip made while the request was in flight must survive.
        let skipped_tag = self.state.skipped_tag.take();
        self.state = CheckState {
            skipped_tag,
            ..state
        };

        match &outcome {
            CheckOutcome::Checked(Some(release)) => {
                log::info!("[Warpium update] Latest Warpium release: {}", release.tag);
            }
            CheckOutcome::Checked(None) => {
                log::info!("[Warpium update] No installable Warpium release is published");
            }
            CheckOutcome::Failed(err) => {
                log::warn!(
                    "[Warpium update] Check failed (attempt {}): {err}",
                    self.state.consecutive_failures
                );
            }
            CheckOutcome::Deferred { until } => {
                log::debug!("[Warpium update] Deferred until {until} (Unix time)");
            }
        }

        if trigger == Trigger::Manual
            && let Some(current) = self.current.clone()
        {
            let newer = match &outcome {
                CheckOutcome::Checked(Some(release)) => release
                    .version()
                    .filter(|version| version > &current)
                    .map(|_| release.clone()),
                CheckOutcome::Checked(None)
                | CheckOutcome::Failed(_)
                | CheckOutcome::Deferred { .. } => None,
            };
            let message = warpium_self_update::manual_report(&current, &outcome, now);
            let toast = match (&outcome, newer) {
                (_, Some(release)) => {
                    // Asking explicitly brings back a skipped or closed offer.
                    self.state.skipped_tag = None;
                    self.dismissed_tag = None;
                    self.manually_found = true;
                    DismissibleToast::default(message).with_link(
                        ToastLink::new("Release notes".to_owned()).with_href(release.page_url()),
                    )
                }
                (CheckOutcome::Checked(_), None) => DismissibleToast::success(message),
                (CheckOutcome::Failed(_) | CheckOutcome::Deferred { .. }, None) => {
                    DismissibleToast::error(message)
                }
            };
            show_toast(toast, ctx);
        }

        self.save_state(ctx);
        self.reschedule(ctx);
        ctx.notify();
    }

    fn save_state(&self, ctx: &mut ModelContext<Self>) {
        if let Err(err) = ctx
            .private_user_preferences()
            .write_value(STATE_KEY, self.state.to_json())
        {
            log::warn!("[Warpium update] Failed to save the check state: {err:#}");
        }
    }

    fn save_install_record(&self, record: Option<&InstallRecord>, ctx: &mut ModelContext<Self>) {
        let preferences = ctx.private_user_preferences();
        let result = match record {
            Some(record) => match serde_json::to_string(record) {
                Ok(json) => preferences.write_value(INSTALL_RECORD_KEY, json),
                Err(err) => {
                    log::warn!("[Warpium update] Failed to serialize the install record: {err}");
                    return;
                }
            },
            None => preferences.remove_value(INSTALL_RECORD_KEY),
        };
        if let Err(err) = result {
            log::warn!("[Warpium update] Failed to save the install record: {err:#}");
        }
    }
}

/// Why "Update now" failed.
#[derive(Debug, thiserror::Error)]
enum UpdateFailure {
    #[error("{0}")]
    Download(DownloadError),
    #[error("{err}")]
    Install { err: InstallError, dmg: PathBuf },
}

impl UpdateFailure {
    fn dmg_to_reveal(&self) -> Option<PathBuf> {
        match self {
            Self::Install { err, dmg } if err.is_location_problem() => Some(dmg.clone()),
            Self::Install { .. } | Self::Download(_) => None,
        }
    }

    fn log(self) {
        match self {
            Self::Download(err) => log::warn!("[Warpium update] Download failed: {err}"),
            Self::Install { err, .. } if err.is_location_problem() => {
                log::warn!("[Warpium update] Can't update in place: {err}");
            }
            // A verified release that doesn't install points at a broken release or a bug here.
            Self::Install { err, .. } => {
                report_error!(
                    anyhow::Error::new(err).context("Failed to install a Warpium update")
                );
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum DownloadError {
    #[error("couldn't download the update: {0}")]
    Transport(String),
    #[error("couldn't download the update: GitHub answered HTTP {0}")]
    Http(u16),
    #[error("{0}")]
    Checksum(#[from] ChecksumError),
    #[error("the download is incomplete ({received} of {expected} bytes)")]
    Incomplete { received: u64, expected: u64 },
    #[error("couldn't save the update: {0}")]
    Io(#[from] std::io::Error),
}

/// Downloads the release's disk image into `dir`, checking it against the release's SHA-256, and
/// returns its path.
async fn download_release(
    client: &http_client::Client,
    release: &Release,
    dir: &Path,
    spawner: &ModelSpawner<SelfUpdater>,
) -> Result<PathBuf, DownloadError> {
    async_fs::create_dir_all(dir).await?;

    let response = client
        .get(release.checksum.url.as_str())
        .header("User-Agent", USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|err| DownloadError::Transport(format!("{err:#}")))?;
    if !response.status().is_success() {
        return Err(DownloadError::Http(response.status().as_u16()));
    }
    let checksum = response
        .bytes()
        .await
        .map_err(|err| DownloadError::Transport(format!("{err:#}")))?;
    let expected = warpium_self_update::parse_checksum_file(
        &String::from_utf8_lossy(&checksum),
        &release.dmg.name,
    )?;

    let response = client
        .get(release.dmg.url.as_str())
        .header("User-Agent", USER_AGENT)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|err| DownloadError::Transport(format!("{err:#}")))?;
    if !response.status().is_success() {
        return Err(DownloadError::Http(response.status().as_u16()));
    }

    let partial = dir.join(format!("{}.part", release.dmg.name));
    let mut file = async_fs::File::create(&partial).await?;
    let mut hasher = Sha256Hasher::default();
    let mut received = 0u64;
    let mut reported = Some(0);
    let mut stream = Box::pin(response.bytes_stream());
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| DownloadError::Transport(format!("{err:#}")))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        received += chunk.len() as u64;
        let percent = warpium_self_update::progress_percent(received, release.dmg.size);
        if percent != reported {
            reported = percent;
            let _ = spawner
                .spawn(move |updater, ctx| updater.set_download_progress(percent, ctx))
                .await;
        }
    }
    file.sync_all().await?;
    drop(file);

    let verified = if release.dmg.size > 0 && received != release.dmg.size {
        Err(DownloadError::Incomplete {
            received,
            expected: release.dmg.size,
        })
    } else {
        hasher.verify(&expected).map_err(DownloadError::from)
    };
    if let Err(err) = verified {
        let _ = async_fs::remove_file(&partial).await;
        return Err(err);
    }

    let dmg = dir.join(&release.dmg.name);
    async_fs::rename(&partial, &dmg).await?;
    Ok(dmg)
}

/// Runs the blocking install on its own thread.
async fn install_off_main_thread(
    dmg: PathBuf,
    version: WarpiumVersion,
    bundle: PathBuf,
    mountpoint: PathBuf,
) -> Result<Installed, InstallError> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = warpium_self_update::install(
            &warpium_self_update::InstallRequest {
                dmg: &dmg,
                version: &version,
                bundle: &bundle,
                mountpoint: &mountpoint,
            },
            &warpium_self_update::MacTools,
        );
        let _ = sender.send(result);
    });
    receiver
        .await
        .unwrap_or_else(|_| Err(InstallError::Swap("the install thread stopped".to_owned())))
}

/// The running app bundle, or a message for the user when there is none to replace.
fn running_bundle_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        warp_core::macos::get_bundle_path()
            .map(PathBuf::from)
            .map_err(|err| format!("couldn't find the running app: {err}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("updating in place is only supported on macOS".to_owned())
    }
}

/// Where downloads and mountpoints live while an update is installed.
fn update_dir() -> PathBuf {
    warp_core::paths::cache_dir().join("warpium-update")
}

fn remove_in_background(dir: PathBuf) {
    std::thread::spawn(move || {
        if let Err(err) = std::fs::remove_dir_all(&dir)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("[Warpium update] Couldn't clean up old downloads: {err}");
        }
    });
}

fn capitalize(message: &str) -> String {
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Warpium updates are a Warpium feature; Warpium is the OSS channel.
fn is_supported_channel() -> bool {
    matches!(ChannelState::channel(), Channel::Oss)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

async fn fetch(
    client: Arc<http_client::Client>,
    request: HttpRequest,
) -> Result<HttpResponse, String> {
    let mut builder = client
        .get(request.url.as_str())
        // GitHub's REST API requires a User-Agent and recommends pinning the media type and
        // version: https://docs.github.com/en/rest/using-the-rest-api/getting-started-with-the-rest-api
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", USER_AGENT)
        .timeout(REQUEST_TIMEOUT);
    if let Some(etag) = &request.if_none_match {
        builder = builder.header("If-None-Match", etag.as_str());
    }
    let response = builder.send().await.map_err(|err| format!("{err:#}"))?;
    let status = response.status().as_u16();
    let [etag, retry_after, rate_limit_remaining, rate_limit_reset] = [
        "etag",
        "retry-after",
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
    ]
    .map(|name| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    });
    let body = response
        .bytes()
        .await
        .map_err(|err| format!("{err:#}"))?
        .to_vec();
    Ok(HttpResponse {
        status,
        etag,
        retry_after,
        rate_limit_remaining,
        rate_limit_reset,
        body,
    })
}
