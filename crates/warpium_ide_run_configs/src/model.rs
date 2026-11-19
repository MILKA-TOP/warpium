//! The imported run configurations.

use warpium_project_toolbar::CommandAction;

/// Where a run configuration was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigOrigin {
    /// A file committed with the project, by its path relative to the project root, such as
    /// `.run/detekt.run.xml`. Its commands need the user's trust.
    Shared(String),
    /// The user's own `.idea/workspace.xml`.
    Personal,
}

impl ConfigOrigin {
    pub fn is_shared(&self) -> bool {
        matches!(self, ConfigOrigin::Shared(_))
    }
}

/// One JetBrains run configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeRunConfig {
    /// Stable across reloads (`ide:<type>:<name>`); remembered as the toolbar's selection.
    pub key: String,
    pub name: String,
    /// The configuration's `type`, such as `GradleRunConfiguration`.
    pub type_id: String,
    pub origin: ConfigOrigin,
    pub run: IdeRun,
}

impl IdeRunConfig {
    pub fn key_for(type_id: &str, name: &str) -> String {
        format!("ide:{type_id}:{name}")
    }

    /// The steps to run, unless the configuration isn't supported.
    pub fn steps(&self) -> Option<&[RunStep]> {
        match &self.run {
            IdeRun::Steps(steps) => Some(steps),
            IdeRun::Unsupported(_) => None,
        }
    }
}

/// What running a configuration does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdeRun {
    /// Run these in order: one step, or the members of a compound configuration.
    Steps(Vec<RunStep>),
    /// Warpium can't run it, with the reason for the user.
    Unsupported(String),
}

/// One thing a run configuration starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStep {
    /// The key of the configuration the step comes from: the configuration itself, or a member
    /// of a compound one.
    pub config_key: String,
    /// The name of that configuration.
    pub label: String,
    /// Whether that configuration comes from a shared file.
    pub shared: bool,
    pub action: StepAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepAction {
    /// A shell command, run like a project toolbar command. Its `cwd` is relative to the project
    /// root (with `..` for folders outside it).
    Command(CommandAction),
    /// Build, install and start an Android application module.
    AndroidApp(AndroidRun),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidRun {
    /// The module's Gradle path, such as `:app`.
    pub gradle_path: String,
    pub launch: AndroidLaunch,
}

/// What an Android run starts once the app is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndroidLaunch {
    DefaultActivity,
    /// This activity, fully qualified or relative to the package (`.MainActivity`).
    Activity(String),
    DeepLink(String),
    /// Only install the app.
    Nothing,
}

/// A project's run configurations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdeRunConfigs {
    /// Shared configurations first, then personal ones, each in file order.
    pub configs: Vec<IdeRunConfig>,
    /// The configuration selected in the IDE (`RunManager`'s `selected`), when it was imported.
    pub default_key: Option<String>,
    /// Files or configurations that couldn't be read, for the user.
    pub warnings: Vec<String>,
}

impl IdeRunConfigs {
    pub fn get(&self, key: &str) -> Option<&IdeRunConfig> {
        self.configs.iter().find(|config| config.key == key)
    }

    pub fn is_empty(&self) -> bool {
        self.configs.is_empty() && self.warnings.is_empty()
    }
}

/// The IDE's name for a configuration type, as in its run configuration editor.
pub fn type_label(type_id: &str) -> &str {
    match type_id {
        "GradleRunConfiguration" => "Gradle",
        "ShConfigurationType" => "Shell Script",
        "AndroidRunConfigurationType" => "Android App",
        "AndroidTestRunConfigurationType" => "Android Instrumented Tests",
        "JUnit" => "JUnit",
        "AndroidJUnit" => "Android JUnit",
        "JetRunConfigurationType" => "Kotlin",
        "KotlinStandaloneScriptRunConfigurationType" => "Kotlin Script",
        "Application" => "Application",
        "CompoundRunConfigurationType" => "Compound",
        other => other,
    }
}
