//! Android projects without Gradle: app modules from `settings.gradle(.kts)` and their build
//! scripts, application ids, and the commands that install and start an app.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::adb::shell_quote;

const SETTINGS_FILES: [&str; 2] = ["settings.gradle.kts", "settings.gradle"];
const BUILD_FILES: [&str; 2] = ["build.gradle.kts", "build.gradle"];

/// A Gradle module that applies the Android application plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidAppModule {
    /// The Gradle path, such as `:app`; empty for a single-module project's root.
    pub gradle_path: String,
    pub dir: PathBuf,
    /// `applicationId` (or `namespace`) from the build script.
    pub application_id: Option<String>,
    /// The debug build type's `applicationIdSuffix`.
    pub debug_suffix: Option<String>,
}

impl AndroidAppModule {
    /// A short name for menus: the last part of the Gradle path, or the folder name.
    pub fn display_name(&self) -> String {
        match self.gradle_path.rsplit(':').next() {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => self
                .dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "app".to_owned()),
        }
    }

    /// The Gradle task that builds and installs `variant` (`debug` gives `:app:installDebug`, or
    /// `:installDebug` for the root project).
    pub fn install_task(&self, variant: &str) -> String {
        format!(
            "{}:install{}",
            self.gradle_path,
            crate::device::capitalize(variant)
        )
    }

    /// AGP's `output-metadata.json` for `variant`, written when the APK is packaged.
    pub fn output_metadata_path(&self, variant: &str) -> PathBuf {
        self.dir
            .join("build/outputs/apk")
            .join(variant)
            .join("output-metadata.json")
    }

    /// The package `variant` installs as: from the last build's `output-metadata.json` when there
    /// is one (exact, flavors and suffixes included), else the build script's id plus the debug
    /// suffix.
    pub fn package_for_variant(&self, variant: &str) -> Option<String> {
        if let Some(id) = std::fs::read_to_string(self.output_metadata_path(variant))
            .ok()
            .and_then(|json| application_id_from_metadata(&json))
        {
            return Some(id);
        }
        let id = self.application_id.clone()?;
        match (&self.debug_suffix, variant) {
            (Some(suffix), "debug") => Some(format!("{id}{suffix}")),
            _ => Some(id),
        }
    }
}

static QUOTED_MODULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"["'](:[\w.:-]+)["']"#).expect("valid pattern"));
static APPLICATION_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bapplicationId\s*=?\s*["']([\w.]+)["']"#).expect("valid pattern")
});
static NAMESPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bnamespace\s*=?\s*["']([\w.]+)["']"#).expect("valid pattern"));
static DEBUG_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)(?:getByName\(\s*"debug"\s*\)|\bdebug)\s*\{[^}]*?\bapplicationIdSuffix\s*=?\s*["']([\w.]+)["']"#,
    )
    .expect("valid pattern")
});
static METADATA_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""applicationId"\s*:\s*"([\w.]+)""#).expect("valid pattern"));

/// Gradle paths of the modules `settings.gradle(.kts)` includes.
pub fn parse_settings_modules(settings: &str) -> Vec<String> {
    let mut modules = Vec::new();
    let mut in_include = false;
    let mut depth = 0i32;
    for line in settings.lines() {
        let code = line.split("//").next().unwrap_or("");
        let trimmed = code.trim();
        if trimmed.starts_with("include") {
            in_include = true;
            depth = 0;
        } else if !in_include {
            continue;
        }
        for captures in QUOTED_MODULE.captures_iter(code) {
            let module = captures[1].to_owned();
            if !modules.contains(&module) {
                modules.push(module);
            }
        }
        // An include ends with its closing parenthesis, or (Groovy) a line without a trailing
        // comma.
        depth += code.matches('(').count() as i32 - code.matches(')').count() as i32;
        if depth <= 0 && !trimmed.ends_with(',') {
            in_include = false;
        }
    }
    modules
}

/// Whether a build script applies the Android application plugin, directly
/// (`com.android.application`), through a version catalog (`libs.plugins.android.application`,
/// `androidApplication`) or a convention plugin (`myapp.android.application`). Declarations with
/// `apply false` (how root build scripts put plugins on the classpath) don't count.
pub fn is_app_build_script(script: &str) -> bool {
    script.lines().any(|line| {
        let code = line.split("//").next().unwrap_or("");
        (code.contains("android.application") || code.contains("androidApplication"))
            && !code.contains("apply false")
    })
}

fn read_build_script(dir: &Path) -> Option<String> {
    BUILD_FILES
        .iter()
        .find_map(|name| std::fs::read_to_string(dir.join(name)).ok())
}

fn app_module(gradle_path: String, dir: PathBuf) -> Option<AndroidAppModule> {
    let script = read_build_script(&dir)?;
    if !is_app_build_script(&script) {
        return None;
    }
    let application_id = APPLICATION_ID
        .captures(&script)
        .or_else(|| NAMESPACE.captures(&script))
        .map(|captures| captures[1].to_owned());
    let debug_suffix = DEBUG_SUFFIX
        .captures(&script)
        .map(|captures| captures[1].to_owned());
    Some(AndroidAppModule {
        gradle_path,
        dir,
        application_id,
        debug_suffix,
    })
}

/// A module's directory by Gradle's default layout (`:feature:home` is `feature/home`).
fn module_dir(root: &Path, gradle_path: &str) -> PathBuf {
    gradle_path
        .split(':')
        .filter(|part| !part.is_empty())
        .fold(root.to_path_buf(), |dir, part| dir.join(part))
}

/// The project's Android application modules, in `settings.gradle` order (the root project
/// first, for single-module projects).
pub fn find_app_modules(root: &Path) -> Vec<AndroidAppModule> {
    let mut modules: Vec<AndroidAppModule> = app_module(String::new(), root.to_path_buf())
        .into_iter()
        .collect();
    let settings = SETTINGS_FILES
        .iter()
        .find_map(|name| std::fs::read_to_string(root.join(name)).ok())
        .unwrap_or_default();
    for gradle_path in parse_settings_modules(&settings) {
        let dir = module_dir(root, &gradle_path);
        if let Some(module) = app_module(gradle_path, dir) {
            modules.push(module);
        }
    }
    modules
}

/// Whether `root` is an Android project: it has an application module, or a module with an
/// `AndroidManifest.xml` (libraries, KMP `androidMain`).
pub fn is_android_project(root: &Path) -> bool {
    if !find_app_modules(root).is_empty() {
        return true;
    }
    let settings = SETTINGS_FILES
        .iter()
        .find_map(|name| std::fs::read_to_string(root.join(name)).ok())
        .unwrap_or_default();
    let module_dirs = std::iter::once(root.to_path_buf()).chain(
        parse_settings_modules(&settings)
            .into_iter()
            .map(|path| module_dir(root, &path)),
    );
    for dir in module_dirs {
        for source_set in ["main", "androidMain"] {
            if dir
                .join("src")
                .join(source_set)
                .join("AndroidManifest.xml")
                .is_file()
            {
                return true;
            }
        }
    }
    false
}

/// The `applicationId` in AGP's `output-metadata.json`.
pub fn application_id_from_metadata(json: &str) -> Option<String> {
    METADATA_ID
        .captures(json)
        .map(|captures| captures[1].to_owned())
}

/// The device shell command that starts `package`'s launcher activity: resolved with
/// `cmd package resolve-activity` (Android 7+), else through `monkey`.
pub fn launch_app_command(package: &str) -> String {
    let package = shell_quote(package);
    format!(
        "a=$(cmd package resolve-activity --brief -c android.intent.category.LAUNCHER {package} \
         2>/dev/null | tail -n 1); case \"$a\" in */*) am start -n \"$a\" -a \
         android.intent.action.MAIN -c android.intent.category.LAUNCHER;; *) monkey -p {package} \
         -c android.intent.category.LAUNCHER 1;; esac"
    )
}

/// The device shell command that starts `activity` of `package`; a name starting with `.` is
/// relative to the package, as in the manifest.
pub fn launch_activity_command(package: &str, activity: &str) -> String {
    format!(
        "am start -n {} -a android.intent.action.MAIN -c android.intent.category.LAUNCHER",
        shell_quote(&format!("{package}/{activity}"))
    )
}

/// The device shell command that opens `link` in `package`.
pub fn launch_deep_link_command(package: &str, link: &str) -> String {
    format!(
        "am start -a android.intent.action.VIEW -c android.intent.category.BROWSABLE -d {} {}",
        shell_quote(link),
        shell_quote(package)
    )
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
