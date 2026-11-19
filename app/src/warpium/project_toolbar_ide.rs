//! Warpium: the project's JetBrains run configurations in the project toolbar's
//! run-configurations menu. Reading and mapping them is the `warpium_ide_run_configs` crate's
//! job; this module reads them off the main thread, lists them, asks for trust and runs them.

use std::path::{Path, PathBuf};

use futures::channel::oneshot;
use warpium_android::run::LaunchTarget;
use warpium_ide_run_configs::{
    AndroidLaunch, ConfigOrigin, IdeRun, IdeRunConfig, StepAction, WORKSPACE_FILE,
    escape_toolbar_variables, is_ide_config_path, load_project, needs_trust, trust_fingerprint,
    trust_review, trust_signatures, type_label,
};
use warpium_project_toolbar::{CommandTarget, RunConfigItem, RunConfigKind};
use warpui::elements::ClippedScrollStateHandle;
use warpui::fonts::Properties;
use warpui::{AppContext, SingletonEntity as _, ViewContext};

use super::{
    AndroidRunRequest, PendingRun, ProjectToolbarAction, ProjectToolbarEvent, ProjectToolbarStore,
    ProjectToolbarView, TrustPrompt, TrustTarget,
};
use crate::menu::{MenuItem, MenuItemFields};
use crate::warpium::sync_settings::WarpiumSettings;

/// How many unreadable files or configurations the menu's warning lists by name.
const MAX_WARNINGS_SHOWN: usize = 5;

/// Whether the toolbar imports IDE run configurations.
pub(super) fn is_enabled(ctx: &AppContext) -> bool {
    super::is_enabled(ctx) && *WarpiumSettings::as_ref(ctx).ide_run_configs
}

impl ProjectToolbarView {
    /// Reads the project's run configurations again, off the main thread.
    pub(super) fn reload_ide_configs(&mut self, ctx: &mut ViewContext<Self>) {
        self.ide_generation += 1;
        let root = self
            .project_root()
            .filter(|_| is_enabled(ctx))
            .map(Path::to_path_buf);
        let Some(root) = root else {
            if !self.ide_configs.is_empty() {
                self.ide_configs = Default::default();
                self.drop_stale_ide_trust_prompt();
                self.refresh_run_configs(ctx);
            }
            self.sync_watched_dirs(ctx);
            return;
        };
        let generation = self.ide_generation;
        let (sender, receiver) = oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("ide-run-configs".to_owned())
            .spawn(move || {
                let home = dirs::home_dir();
                let _ = sender.send(load_project(&root, home.as_deref()));
            });
        if let Err(err) = spawned {
            log::warn!("[Project toolbar] Couldn't start reading IDE run configurations: {err}");
            return;
        }
        ctx.spawn(async move { receiver.await.ok() }, move |me, configs, ctx| {
            if me.ide_generation != generation {
                return;
            }
            let configs = configs.unwrap_or_default();
            if !configs.warnings.is_empty() {
                let count = configs.warnings.len();
                let warnings = configs.warnings.join("; ");
                warp_core::safe_warn!(
                    safe: ("[Project toolbar] {count} IDE run configuration files or entries couldn't be read"),
                    full: ("[Project toolbar] Couldn't read IDE run configurations: {warnings}")
                );
            }
            me.ide_configs = configs;
            me.drop_stale_ide_trust_prompt();
            me.sync_watched_dirs(ctx);
            me.refresh_run_configs(ctx);
        });
    }

    pub(super) fn is_ide_config_change(&self, paths: &[PathBuf]) -> bool {
        self.project_root()
            .is_some_and(|root| paths.iter().any(|path| is_ide_config_path(root, path)))
    }

    /// Whether the run-configurations widget is shown: there is something to run, or IDE run
    /// configurations to explain.
    pub(super) fn shows_run_widget(&self) -> bool {
        !self.run_configs.is_empty() || !self.ide_configs.is_empty()
    }

    /// The run-configurations entries of the IDE run configurations that can run.
    pub(super) fn ide_run_config_items(&self, ctx: &AppContext) -> Vec<RunConfigItem> {
        self.ide_configs
            .configs
            .iter()
            .filter(|config| self.ide_unavailable_reason(config, ctx).is_none())
            .map(|config| RunConfigItem {
                key: config.key.clone(),
                label: config.name.clone(),
                kind: RunConfigKind::IdeConfig(config.key.clone()),
            })
            .collect()
    }

    /// Why `config` can't run here, if it can't.
    fn ide_unavailable_reason(&self, config: &IdeRunConfig, ctx: &AppContext) -> Option<String> {
        match &config.run {
            IdeRun::Unsupported(reason) => Some(reason.clone()),
            IdeRun::Steps(steps) => {
                let runs_android = steps
                    .iter()
                    .any(|step| matches!(step.action, StepAction::AndroidApp(_)));
                (runs_android && !crate::warpium::android::is_enabled(ctx)).then(|| {
                    "Android configurations run with Warpium's Android tools, which are off"
                        .to_owned()
                })
            }
        }
    }

    /// The "IDE run configurations" section of the run-configurations menu: every imported
    /// configuration (those that can't run are disabled, with the reason in their tooltip), then
    /// a warning about files or configurations that couldn't be read.
    pub(super) fn ide_menu_items(&self, ctx: &AppContext) -> Vec<MenuItem<ProjectToolbarAction>> {
        if self.ide_configs.is_empty() {
            return Vec::new();
        }
        let mut items = vec![
            MenuItem::Separator,
            MenuItem::Header {
                fields: MenuItemFields::new("IDE run configurations")
                    .with_no_interaction_on_hover(),
                clickable: false,
                right_side_fields: None,
            },
        ];
        for config in &self.ide_configs.configs {
            let (origin, source) = match &config.origin {
                ConfigOrigin::Shared(file) => ("shared", format!("Shared, from {file}")),
                ConfigOrigin::Personal => ("personal", format!("Personal, from {WORKSPACE_FILE}")),
            };
            let fields = MenuItemFields::new(config.name.clone()).with_right_side_label(
                format!("{} · {origin}", type_label(&config.type_id)),
                Properties::default(),
            );
            let fields = match self.ide_unavailable_reason(config, ctx) {
                Some(reason) => fields
                    .with_disabled(true)
                    .with_tooltip(format!("{source}. Can't run here: {reason}.")),
                None => fields.with_tooltip(source).with_on_select_action(
                    ProjectToolbarAction::SelectRunConfig(config.key.clone()),
                ),
            };
            items.push(MenuItem::Item(fields));
        }
        let warnings = &self.ide_configs.warnings;
        if !warnings.is_empty() {
            let mut tooltip = warnings
                .iter()
                .take(MAX_WARNINGS_SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            if warnings.len() > MAX_WARNINGS_SHOWN {
                tooltip.push_str(&format!(
                    "\n… and {} more",
                    warnings.len() - MAX_WARNINGS_SHOWN
                ));
            }
            items.push(MenuItem::Item(
                MenuItemFields::new(format!(
                    "Couldn't read {} run configuration {}",
                    warnings.len(),
                    if warnings.len() == 1 {
                        "entry"
                    } else {
                        "entries"
                    }
                ))
                .with_disabled(true)
                .with_tooltip(tooltip),
            ));
        }
        items
    }

    /// Runs the IDE run configuration `key`, after asking to trust shared commands (and the
    /// Gradle build, for Android runs) when needed.
    pub(super) fn run_ide_config(&mut self, key: &str, ctx: &mut ViewContext<Self>) {
        let (Some(root), Some(config)) = (
            self.project_root().map(Path::to_path_buf),
            self.ide_configs.get(key).cloned(),
        ) else {
            return;
        };
        if let Some(reason) = self.ide_unavailable_reason(&config, ctx) {
            ctx.emit(ProjectToolbarEvent::ShowError(format!(
                "Can't run \"{}\": {reason}",
                config.name
            )));
            return;
        }
        let Some(steps) = config.steps() else {
            return;
        };
        let state = ProjectToolbarStore::as_ref(ctx).state();
        if needs_trust(&config)
            && let Some(fingerprint) = trust_fingerprint(&self.ide_configs.configs)
            && !state.trusts_ide_run_configs(&root, &fingerprint)
        {
            let review = trust_review(&self.ide_configs.configs, state.trusted_ide_commands(&root));
            self.trust_prompt = Some(TrustPrompt {
                target: TrustTarget::IdeConfigs {
                    fingerprint,
                    key: key.to_owned(),
                },
                review,
                list_scroll: ClippedScrollStateHandle::default(),
            });
            ctx.notify();
            return;
        }
        let mut modules = Vec::new();
        for step in steps {
            if let StepAction::AndroidApp(run) = &step.action {
                match self.android_app(&run.gradle_path) {
                    Some(module) => modules.push(module.clone()),
                    None => {
                        ctx.emit(ProjectToolbarEvent::ShowError(format!(
                            "Can't run \"{}\": Warpium didn't find an Android application \
                             module {} in this project",
                            step.label,
                            if run.gradle_path.is_empty() {
                                "at its root"
                            } else {
                                run.gradle_path.as_str()
                            }
                        )));
                        return;
                    }
                }
            }
        }
        if !modules.is_empty()
            && !ProjectToolbarStore::as_ref(ctx)
                .state()
                .trusts_android_builds(&root)
        {
            let modules = modules.iter().collect::<Vec<_>>();
            self.ask_to_trust_android_build(
                &root,
                &modules,
                PendingRun::IdeConfig(key.to_owned()),
                ctx,
            );
            return;
        }
        for (index, step) in steps.iter().enumerate() {
            match &step.action {
                StepAction::Command(command) => {
                    let mut command = command.clone();
                    command.command = escape_toolbar_variables(&command.command);
                    // Each member of a compound configuration gets a session of its own, as it
                    // gets a tab of its own in the IDE.
                    if index > 0 {
                        command.target = CommandTarget::NewTab;
                    }
                    ctx.emit(ProjectToolbarEvent::RunIdeCommand {
                        root: root.clone(),
                        title: step.label.clone(),
                        command,
                    });
                }
                StepAction::AndroidApp(run) => self.run_android_app(
                    AndroidRunRequest {
                        gradle_path: run.gradle_path.clone(),
                        launch: launch_target(&run.launch),
                    },
                    ctx,
                ),
            }
        }
        ctx.notify();
    }

    /// Trusts the shared commands the prompt listed, unless they changed while it was open,
    /// then runs the configuration it was opened for.
    pub(super) fn confirm_ide_trust(
        &mut self,
        root: &Path,
        fingerprint: &str,
        key: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        if trust_fingerprint(&self.ide_configs.configs).as_deref() != Some(fingerprint) {
            ctx.notify();
            return;
        }
        let signatures = trust_signatures(&self.ide_configs.configs);
        ProjectToolbarStore::handle(ctx).update(ctx, |store, ctx| {
            store.update_state(
                |state| state.trust_ide_run_configs(root, fingerprint, signatures),
                ctx,
            );
        });
        self.run_ide_config(key, ctx);
    }

    /// Closes a trust prompt for IDE run configurations that changed or went away since it
    /// opened.
    fn drop_stale_ide_trust_prompt(&mut self) {
        if let Some(TrustPrompt {
            target: TrustTarget::IdeConfigs { fingerprint, key },
            ..
        }) = &self.trust_prompt
            && (self.ide_configs.get(key).is_none()
                || trust_fingerprint(&self.ide_configs.configs).as_deref()
                    != Some(fingerprint.as_str()))
        {
            self.trust_prompt = None;
        }
    }
}

/// What an Android run starts once the app is installed; `None` only installs it.
fn launch_target(launch: &AndroidLaunch) -> Option<LaunchTarget> {
    match launch {
        AndroidLaunch::DefaultActivity => Some(LaunchTarget::DefaultActivity),
        AndroidLaunch::Activity(activity) => Some(LaunchTarget::Activity(activity.clone())),
        AndroidLaunch::DeepLink(link) => Some(LaunchTarget::DeepLink(link.clone())),
        AndroidLaunch::Nothing => None,
    }
}
