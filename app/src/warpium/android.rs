//! Android devices for Warpium: the live device list (from the adb server's `track-devices`),
//! the selected device, AVDs and launching them. The adb work runs on background threads owned
//! by `warpium_android`; this model only applies their results on the main thread.
//!
//! Nothing touches adb until something needs devices (the logcat pane, the device picker or a
//! palette command), so projects without Android never start an adb server.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::channel::oneshot;
use warp_core::user_preferences::GetUserPreferences as _;
use warpium_android::adb::server_port_from_env;
use warpium_android::avd::{
    Avd, avd_dirs_from_env, launch_avd, list_avds, list_avds_with_emulator, wait_for_avd_boot,
};
use warpium_android::sdk::{AndroidSdk, SdkEnv, find_sdk};
use warpium_android::tracker::{TrackerEvent, spawn_device_tracker};
use warpium_android::{AdbClient, Device, WorkerHandle};
use warpui::keymap::EditableBinding;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use super::is_warpium_channel;
use super::sync_settings::{ANDROID_CONTEXT_FLAG, WarpiumSettings, WarpiumSettingsChangedEvent};
use crate::settings_view::ToggleSettingActionPair;
use crate::util::bindings::BindingGroup;
use crate::view_components::DismissibleToast;
use crate::workspace::WorkspaceAction;

/// Private-preferences key holding the serial of the last selected device.
const SELECTED_DEVICE_KEY: &str = "WarpiumAndroidSelectedDevice";
const BOOT_TIMEOUT: Duration = Duration::from_secs(300);
const BOOT_POLL: Duration = Duration::from_secs(1);

/// Whether the adb server could be reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AdbStatus {
    NotStarted,
    Connecting,
    Connected,
    /// The server couldn't be reached (the message says why); retried in the background.
    Unavailable(String),
}

pub(crate) enum AndroidEvent {
    DevicesChanged,
    SelectionChanged,
    AvdsChanged,
    /// An AVD launched from Warpium finished booting.
    AvdBooted {
        avd: String,
        serial: String,
    },
    /// An AVD launched from Warpium didn't boot.
    AvdLaunchFailed {
        avd: String,
    },
}

pub(crate) struct AndroidModel {
    sdk: Option<AndroidSdk>,
    client: Option<AdbClient>,
    tracker: Option<WorkerHandle>,
    /// Bumped when tracking restarts, so events from an older tracker are dropped.
    generation: u64,
    status: AdbStatus,
    devices: Vec<Device>,
    selected_serial: Option<String>,
    avds: Vec<Avd>,
    /// AVDs being launched and waited for.
    launching: HashSet<String>,
    /// Set to stop pending boot waits when the feature is turned off.
    cancel_launches: Arc<AtomicBool>,
}

impl Entity for AndroidModel {
    type Event = AndroidEvent;
}

impl SingletonEntity for AndroidModel {}

pub(crate) fn init(ctx: &mut AppContext) {
    ctx.add_singleton_model(|_| super::logcat::LogcatPaneManager::default());
    ctx.add_singleton_model(|ctx: &mut ModelContext<AndroidModel>| {
        ctx.subscribe_to_model(
            &WarpiumSettings::handle(ctx),
            |model, _, event, ctx| match event {
                WarpiumSettingsChangedEvent::AndroidTools { .. } => {
                    if !is_enabled(ctx) {
                        model.stop(ctx);
                    }
                }
                WarpiumSettingsChangedEvent::SyncFromWarp { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheck { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheckIntervalHours { .. }
                | WarpiumSettingsChangedEvent::UpstreamCheckMinCommits { .. }
                | WarpiumSettingsChangedEvent::UpdateCheck { .. }
                | WarpiumSettingsChangedEvent::UpdateCheckIntervalHours { .. }
                | WarpiumSettingsChangedEvent::ProjectToolbar { .. }
                | WarpiumSettingsChangedEvent::CodeIndex { .. } => {}
            },
        );
        let selected_serial = ctx
            .private_user_preferences()
            .read_value(SELECTED_DEVICE_KEY)
            .ok()
            .flatten();
        AndroidModel {
            sdk: None,
            client: None,
            tracker: None,
            generation: 0,
            status: AdbStatus::NotStarted,
            devices: Vec::new(),
            selected_serial,
            avds: Vec::new(),
            launching: HashSet::new(),
            cancel_launches: Arc::new(AtomicBool::new(false)),
        }
    });
}

/// Registers the Android palette commands and the setting's palette toggle.
pub(crate) fn init_bindings(ctx: &mut AppContext) {
    use warpui::keymap::macros::*;

    ctx.register_editable_bindings([
        EditableBinding::new(
            "workspace:open_android_logcat",
            "Open Logcat",
            WorkspaceAction::OpenAndroidLogcat,
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(id!("Workspace") & id!(ANDROID_CONTEXT_FLAG)),
        EditableBinding::new(
            "workspace:select_android_device",
            "Select Android device",
            WorkspaceAction::SelectAndroidDevice,
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(id!("Workspace") & id!(ANDROID_CONTEXT_FLAG)),
        EditableBinding::new(
            "workspace:launch_android_avd",
            "Launch Android Virtual Device (AVD)",
            WorkspaceAction::LaunchAndroidAvd,
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(id!("Workspace") & id!(ANDROID_CONTEXT_FLAG)),
    ]);

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "Android tools (devices, logcat, run)",
            WorkspaceAction::ToggleAndroidTools,
            &id!("Workspace"),
            ANDROID_CONTEXT_FLAG,
        )],
        ctx,
    );
}

/// Whether the Android tools are available: Warpium builds with `warpium.android` on.
pub(crate) fn is_enabled(ctx: &AppContext) -> bool {
    is_warpium_channel() && *WarpiumSettings::as_ref(ctx).android_tools
}

impl AndroidModel {
    pub(crate) fn status(&self) -> &AdbStatus {
        &self.status
    }

    pub(crate) fn devices(&self) -> &[Device] {
        &self.devices
    }

    pub(crate) fn avds(&self) -> &[Avd] {
        &self.avds
    }

    pub(crate) fn client(&self) -> Option<&AdbClient> {
        self.client.as_ref()
    }

    pub(crate) fn sdk(&self) -> Option<&AndroidSdk> {
        self.sdk.as_ref()
    }

    pub(crate) fn is_launching(&self, avd: &str) -> bool {
        self.launching.contains(avd)
    }

    pub(crate) fn selected_serial(&self) -> Option<&str> {
        self.selected_serial.as_deref()
    }

    /// The selected device, if it is connected.
    pub(crate) fn selected_device(&self) -> Option<&Device> {
        let serial = self.selected_serial.as_deref()?;
        self.devices.iter().find(|device| device.serial() == serial)
    }

    /// The online emulator running `avd`, if any.
    pub(crate) fn device_running_avd(&self, avd: &str) -> Option<&Device> {
        self.devices.iter().find(|device| {
            device.entry.state.is_online()
                && device
                    .props
                    .as_ref()
                    .and_then(|props| props.avd_name.as_deref())
                    == Some(avd)
        })
    }

    /// Starts discovering the SDK and tracking devices, once. `project_root` may name an SDK in
    /// its `local.properties`.
    pub(crate) fn ensure_started(
        &mut self,
        project_root: Option<&Path>,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.tracker.is_some() || !is_enabled(ctx) {
            return;
        }
        self.sdk = find_sdk(project_root, &SdkEnv::from_process());
        if self.sdk.is_none() {
            log::info!(
                "[Android] No Android SDK found; using a running adb server if there is one"
            );
        }
        let client = AdbClient::new(
            server_port_from_env(),
            self.sdk.as_ref().map(|sdk| sdk.adb.clone()),
        );
        self.client = Some(client.clone());
        self.generation += 1;
        let generation = self.generation;
        let (sender, receiver) = async_channel::unbounded();
        match spawn_device_tracker(client, move |event| {
            let _ = sender.try_send(event);
        }) {
            Ok(tracker) => self.tracker = Some(tracker),
            Err(err) => {
                log::error!("[Android] Couldn't start device tracking: {err}");
                self.status = AdbStatus::Unavailable(err.to_string());
                return;
            }
        }
        self.status = AdbStatus::Connecting;
        self.cancel_launches = Arc::new(AtomicBool::new(false));
        ctx.spawn_stream_local(
            receiver,
            move |model, event, ctx| {
                if model.generation == generation {
                    model.on_tracker_event(event, ctx);
                }
            },
            |_, _| {},
        );
        self.refresh_avds(ctx);
        ctx.notify();
    }

    fn stop(&mut self, ctx: &mut ModelContext<Self>) {
        self.tracker = None;
        self.generation += 1;
        self.status = AdbStatus::NotStarted;
        self.devices.clear();
        self.cancel_launches.store(true, Ordering::Release);
        self.launching.clear();
        ctx.emit(AndroidEvent::DevicesChanged);
        ctx.notify();
    }

    fn on_tracker_event(&mut self, event: TrackerEvent, ctx: &mut ModelContext<Self>) {
        match event {
            TrackerEvent::Devices(devices) => {
                self.status = AdbStatus::Connected;
                self.devices = devices;
                let selected_present = self
                    .selected_serial
                    .as_deref()
                    .is_some_and(|serial| self.devices.iter().any(|d| d.serial() == serial));
                if !selected_present
                    && let Some(online) = self
                        .devices
                        .iter()
                        .find(|device| device.entry.state.is_online())
                        .map(|device| device.serial().to_owned())
                {
                    self.select(online, ctx);
                }
                ctx.emit(AndroidEvent::DevicesChanged);
            }
            TrackerEvent::Unavailable(message) => {
                self.status = AdbStatus::Unavailable(message);
                if !self.devices.is_empty() {
                    self.devices.clear();
                    ctx.emit(AndroidEvent::DevicesChanged);
                }
            }
        }
        ctx.notify();
    }

    /// Makes `serial` the device that runs and logcat use.
    pub(crate) fn select(&mut self, serial: String, ctx: &mut ModelContext<Self>) {
        if self.selected_serial.as_deref() == Some(serial.as_str()) {
            return;
        }
        if let Err(err) = ctx
            .private_user_preferences()
            .write_value(SELECTED_DEVICE_KEY, serial.clone())
        {
            log::warn!("[Android] Failed to save the selected device: {err:#}");
        }
        self.selected_serial = Some(serial);
        ctx.emit(AndroidEvent::SelectionChanged);
        ctx.notify();
    }

    /// Re-reads the AVD list, off the main thread.
    pub(crate) fn refresh_avds(&mut self, ctx: &mut ModelContext<Self>) {
        let emulator = self.sdk.as_ref().and_then(|sdk| sdk.emulator.clone());
        let (sender, receiver) = oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("android-avds".to_owned())
            .spawn(move || {
                let mut avds = list_avds(&avd_dirs_from_env());
                if avds.is_empty()
                    && let Some(emulator) = emulator
                {
                    avds = list_avds_with_emulator(&emulator)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|name| Avd {
                            name,
                            display_name: None,
                            api_level: None,
                        })
                        .collect();
                }
                let _ = sender.send(avds);
            });
        if let Err(err) = spawned {
            log::error!("[Android] Couldn't start listing AVDs: {err}");
            return;
        }
        ctx.spawn(async move { receiver.await.ok() }, |model, avds, ctx| {
            if let Some(avds) = avds {
                model.avds = avds;
                ctx.emit(AndroidEvent::AvdsChanged);
                ctx.notify();
            }
        });
    }

    /// Selects the emulator running `avd`, launching it in its own window first if it isn't
    /// running. [`AndroidEvent::AvdBooted`] follows once it has booted.
    pub(crate) fn launch_avd(&mut self, avd: String, ctx: &mut ModelContext<Self>) {
        if let Some(serial) = self.device_running_avd(&avd).map(|d| d.serial().to_owned()) {
            self.select(serial.clone(), ctx);
            ctx.emit(AndroidEvent::AvdBooted { avd, serial });
            return;
        }
        if self.launching.contains(&avd) {
            return;
        }
        let (Some(client), Some(emulator)) = (
            self.client.clone(),
            self.sdk.as_ref().and_then(|sdk| sdk.emulator.clone()),
        ) else {
            super::show_toast(
                DismissibleToast::error(
                    "The Android emulator isn't installed. Install it with Android Studio's SDK \
                     Manager."
                        .to_owned(),
                ),
                ctx,
            );
            return;
        };
        self.launching.insert(avd.clone());
        let cancelled = self.cancel_launches.clone();
        let thread_avd = avd.clone();
        let (sender, receiver) = oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("android-avd-launch".to_owned())
            .spawn(move || {
                let mut child = match launch_avd(&emulator, &thread_avd) {
                    Ok(child) => child,
                    Err(err) => {
                        let _ = sender.send(Err(format!("couldn't start the emulator: {err}")));
                        return;
                    }
                };
                let result = wait_for_avd_boot(
                    &client,
                    &thread_avd,
                    Some(&mut child),
                    BOOT_TIMEOUT,
                    BOOT_POLL,
                    &|| cancelled.load(Ordering::Acquire),
                );
                let _ = sender.send(result.map_err(|err| err.to_string()));
                // Reap the emulator when it exits; it keeps running in its own window.
                let _ = child.wait();
            });
        if let Err(err) = spawned {
            self.launching.remove(&avd);
            log::error!("[Android] Couldn't start launching an AVD: {err}");
            return;
        }
        ctx.notify();
        ctx.spawn(
            async move { receiver.await.ok() },
            move |model, result, ctx| {
                if !model.launching.remove(&avd) {
                    return;
                }
                match result {
                    Some(Ok(serial)) => {
                        model.select(serial.clone(), ctx);
                        ctx.emit(AndroidEvent::AvdBooted { avd, serial });
                    }
                    Some(Err(message)) => {
                        log::warn!("[Android] The AVD didn't boot: {message}");
                        super::show_toast(
                            DismissibleToast::error(format!("{avd}: {message}")),
                            ctx,
                        );
                        ctx.emit(AndroidEvent::AvdLaunchFailed { avd });
                    }
                    None => ctx.emit(AndroidEvent::AvdLaunchFailed { avd }),
                }
                ctx.notify();
            },
        );
    }
}
