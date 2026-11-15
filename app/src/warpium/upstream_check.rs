//! Notices when upstream Warp (`warpdotdev/warp`) has commits this build lacks, so the fork can be
//! synced.
//!
//! On the OSS channel, while [`WarpiumSettings::upstream_check`] is on, this checks on launch and
//! then every `warpium.upstream_check_interval_hours`, and the workspace shows a banner when the
//! fork is behind. The protocol and decisions live in the `warpium_upstream_check` crate; this
//! module supplies the HTTP client, the schedule, persistence and the UI hooks.

use std::sync::Arc;
use std::time::Duration;

use warp_core::user_preferences::GetUserPreferences as _;
use warpium_upstream_check::{CheckOutcome, CheckState, Trigger, UpstreamStatus};
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::keymap::EditableBinding;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};
use warpui_extras::user_preferences::UserPreferences as _;

use super::sync_settings::{
    UPSTREAM_CHECK_CONTEXT_FLAG, WarpiumSettings, WarpiumSettingsChangedEvent,
};
use super::{fetch_github, is_warpium_channel, show_toast, unix_now};
use crate::settings_view::ToggleSettingActionPair;
use crate::util::bindings::BindingGroup;
use crate::view_components::{DismissibleToast, ToastLink};
use crate::workspace::WorkspaceAction;

/// The upstream commit this build is based on, set by CI (see `script/warpium/upstream_base`).
const EMBEDDED_BASE: Option<&str> = option_env!("WARPIUM_UPSTREAM_BASE");
/// Overrides [`EMBEDDED_BASE`] at runtime, e.g. to try the notification with an older commit. A
/// separate name so a value exported for a local build can't leak into later launches.
const BASE_OVERRIDE_ENV: &str = "WARPIUM_UPSTREAM_BASE_OVERRIDE";
/// Private-preferences key holding the serialized [`CheckState`].
const STATE_KEY: &str = "WarpiumUpstreamCheckState";

/// Checks upstream Warp for new commits and holds the latest result.
pub(crate) struct UpstreamChecker {
    /// The upstream commit to compare against; `None` when the build doesn't know it.
    base: Option<String>,
    /// Remembered across launches; also holds the latest result.
    state: CheckState,
    running: bool,
    /// Whether this launch runs scheduled checks.
    polling: bool,
    next_check: Option<SpawnedFutureHandle>,
    client: Arc<http_client::Client>,
}

impl Entity for UpstreamChecker {
    type Event = ();
}

impl SingletonEntity for UpstreamChecker {}

/// Registers the checker and, for `poll` launches, starts the scheduled checks. The checker is
/// registered in every launch mode because the command palette action is.
pub(crate) fn init(poll: bool, ctx: &mut AppContext) {
    let checker = ctx.add_singleton_model(|ctx: &mut ModelContext<UpstreamChecker>| {
        ctx.subscribe_to_model(
            &WarpiumSettings::handle(ctx),
            |checker, _, event, ctx| match event {
                WarpiumSettingsChangedEvent::UpstreamCheck { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheckIntervalHours { .. } => {
                    checker.reschedule(ctx);
                    ctx.notify();
                }
                WarpiumSettingsChangedEvent::UpstreamCheckMinCommits { .. } => ctx.notify(),
                WarpiumSettingsChangedEvent::SyncFromWarp { .. }
                | WarpiumSettingsChangedEvent::UpdateCheck { .. }
                | WarpiumSettingsChangedEvent::UpdateCheckIntervalHours { .. }
                | WarpiumSettingsChangedEvent::ProjectToolbar { .. }
                | WarpiumSettingsChangedEvent::CodeIndex { .. } => {}
            },
        );
        UpstreamChecker::new(poll, ctx)
    });
    if poll {
        checker.update(ctx, |checker, ctx| checker.reschedule(ctx));
    }
}

/// Registers the command palette actions for upstream checks.
pub(crate) fn init_bindings(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_editable_bindings([EditableBinding::new(
        "workspace:check_for_upstream_warp_updates",
        "Check for upstream Warp updates",
        WorkspaceAction::CheckForUpstreamWarpUpdates,
    )
    .with_group(BindingGroup::Settings.as_str())
    .with_context_predicate(id!("Workspace"))]);

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "upstream Warp update notifications",
            WorkspaceAction::ToggleUpstreamCheck,
            &id!("Workspace"),
            UPSTREAM_CHECK_CONTEXT_FLAG,
        )],
        app,
    );
}

impl UpstreamChecker {
    fn new(polling: bool, ctx: &mut ModelContext<Self>) -> Self {
        let runtime_override = std::env::var(BASE_OVERRIDE_ENV).ok();
        let base = warpium_upstream_check::resolve_base(runtime_override.as_deref(), EMBEDDED_BASE);
        if base.is_none() && is_warpium_channel() {
            log::info!(
                "[Upstream check] This build has no upstream base commit; upstream checks are off"
            );
        }
        let state = ctx
            .private_user_preferences()
            .read_value(STATE_KEY)
            .ok()
            .flatten()
            .map(|json| CheckState::from_json(&json))
            .unwrap_or_default();
        Self {
            base,
            state,
            running: false,
            polling,
            next_check: None,
            client: Arc::new(http_client::Client::new_external()),
        }
    }

    /// The upstream status to notify about, if any.
    pub(crate) fn notice(&self, ctx: &AppContext) -> Option<UpstreamStatus> {
        if !self.is_enabled(ctx) {
            return None;
        }
        let min_commits = *WarpiumSettings::as_ref(ctx).upstream_check_min_commits;
        self.latest().filter(|status| {
            warpium_upstream_check::should_notify(
                status,
                self.state.dismissed_head.as_deref(),
                min_commits,
            )
        })
    }

    /// The latest known result for this build's base, possibly from an earlier launch.
    fn latest(&self) -> Option<UpstreamStatus> {
        self.state.cached_status(self.base.as_deref()?)
    }

    /// The GitHub page listing the upstream commits this build lacks.
    pub(crate) fn compare_url(&self) -> Option<String> {
        self.base
            .as_deref()
            .map(warpium_upstream_check::compare_web_url)
    }

    /// Hides the notification until upstream moves past the current head.
    pub(crate) fn dismiss(&mut self, ctx: &mut ModelContext<Self>) {
        let Some(status) = self.latest() else {
            return;
        };
        self.state.dismissed_head = Some(status.head_sha);
        self.save_state(ctx);
        ctx.notify();
    }

    /// Checks now and reports the result in a toast, whatever it is.
    pub(crate) fn check_now(&mut self, ctx: &mut ModelContext<Self>) {
        if !is_warpium_channel() {
            show_toast(
                DismissibleToast::default(
                    "Upstream Warp checks are only available in Warpium builds".to_owned(),
                ),
                ctx,
            );
            return;
        }
        if self.base.is_none() {
            show_toast(
                DismissibleToast::default(
                    "This build doesn't record the upstream Warp commit it is based on, so it \
                     can't check for upstream updates"
                        .to_owned(),
                ),
                ctx,
            );
            return;
        }
        if self.running {
            show_toast(
                DismissibleToast::default(
                    "A check for upstream Warp updates is already running".to_owned(),
                ),
                ctx,
            );
            return;
        }
        self.run_check(Trigger::Manual, ctx);
    }

    fn is_enabled(&self, ctx: &AppContext) -> bool {
        is_warpium_channel() && self.base.is_some() && *WarpiumSettings::as_ref(ctx).upstream_check
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
        let Some(base) = &self.base else {
            return;
        };
        let delay = warpium_upstream_check::next_check_delay(
            &self.state,
            base,
            unix_now(),
            self.interval(ctx),
        );
        self.next_check = Some(ctx.spawn(
            async move {
                Timer::after(delay).await;
            },
            |checker, _, ctx| {
                checker.next_check = None;
                if checker.running {
                    return;
                }
                checker.run_check(Trigger::Scheduled, ctx);
            },
        ));
    }

    fn interval(&self, ctx: &AppContext) -> Duration {
        warpium_upstream_check::check_interval(
            *WarpiumSettings::as_ref(ctx).upstream_check_interval_hours,
        )
    }

    fn run_check(&mut self, trigger: Trigger, ctx: &mut ModelContext<Self>) {
        let Some(base) = self.base.clone() else {
            return;
        };
        self.running = true;
        let state = self.state.clone();
        let client = self.client.clone();
        let max_backoff = self.interval(ctx);
        ctx.spawn(
            async move {
                let now = unix_now();
                let (state, outcome) = warpium_upstream_check::run_check(
                    state,
                    &base,
                    now,
                    max_backoff,
                    trigger,
                    |request| fetch_github(client.clone(), request),
                )
                .await;
                (state, outcome, now)
            },
            move |checker, (state, outcome, now), ctx| {
                checker.finish_check(trigger, state, outcome, now, ctx);
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
        self.running = false;
        // A dismissal made while the request was in flight must survive.
        let dismissed_head = self.state.dismissed_head.take();
        self.state = CheckState {
            dismissed_head,
            ..state
        };
        self.save_state(ctx);

        match &outcome {
            CheckOutcome::Checked(status) => {
                log::info!(
                    "[Upstream check] Upstream Warp is {} commits ahead of base {} (head {})",
                    status.ahead_by,
                    status.base,
                    status.head_sha
                );
            }
            CheckOutcome::Failed(err) => {
                log::warn!(
                    "[Upstream check] Check failed (attempt {}): {err}",
                    self.state.consecutive_failures
                );
            }
            CheckOutcome::Deferred { until } => {
                log::debug!("[Upstream check] Deferred until {until} (Unix time)");
            }
        }

        if trigger == Trigger::Manual {
            let message = warpium_upstream_check::manual_report(&outcome, now);
            let toast = match &outcome {
                CheckOutcome::Checked(status) if status.ahead_by > 0 => {
                    DismissibleToast::default(message).with_link(
                        ToastLink::new("View changes".to_owned())
                            .with_href(warpium_upstream_check::compare_web_url(&status.base)),
                    )
                }
                CheckOutcome::Checked(_) => DismissibleToast::success(message),
                CheckOutcome::Failed(_) | CheckOutcome::Deferred { .. } => {
                    DismissibleToast::error(message)
                }
            };
            show_toast(toast, ctx);
        }

        self.reschedule(ctx);
        ctx.notify();
    }

    fn save_state(&self, ctx: &mut ModelContext<Self>) {
        if let Err(err) = ctx
            .private_user_preferences()
            .write_value(STATE_KEY, self.state.to_json())
        {
            log::warn!("[Upstream check] Failed to save the check state: {err:#}");
        }
    }
}
