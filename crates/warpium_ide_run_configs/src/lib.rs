//! Warpium: JetBrains run configurations (Android Studio, IntelliJ IDEA) of a project, mapped to
//! what the project toolbar can run.
//!
//! Shared configurations come from the committed `.run/*.run.xml` and
//! `.idea/runConfigurations/*.xml`; personal ones from the `RunManager` component of
//! `.idea/workspace.xml`. The files are only ever read. Each configuration becomes either a list
//! of steps (shell commands, or Android app runs handled by the app's Android flow) or an
//! unsupported entry with the reason. The app supplies the UI, the file watching, trust storage
//! and the execution.

mod load;
mod macros;
mod mapping;
mod model;
mod module;
mod shell;
mod trust;
mod xml;

pub use load::{
    IDEA_DIR, RUN_CONFIGURATIONS_DIR, RUN_DIR, WORKSPACE_FILE, is_ide_config_path, load_project,
    watched_dirs,
};
pub use model::{
    AndroidLaunch, AndroidRun, ConfigOrigin, IdeRun, IdeRunConfig, IdeRunConfigs, RunStep,
    StepAction, type_label,
};
pub use shell::escape_toolbar_variables;
pub use trust::{needs_trust, trust_fingerprint, trust_review, trust_signatures};
