//! IDE module names (`MyApp.app.main`, `MyApp.core.data.unitTest`) to Gradle projects, and
//! finding the Gradle launcher.

use std::path::{Path, PathBuf};

/// The Gradle project an IDE module belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GradleModule {
    /// Such as `:core:data`; empty for the root project.
    pub(crate) gradle_path: String,
    pub(crate) dir: PathBuf,
    /// The source set the IDE module stands for (`main`, `test`, `unitTest`, `jvmTest`, …), when
    /// the name has one.
    pub(crate) source_set: Option<String>,
}

impl GradleModule {
    /// The task path of `task` in this project (`:core:data:test`, or `:test` for the root).
    pub(crate) fn task(&self, task: &str) -> String {
        format!("{}:{task}", self.gradle_path)
    }
}

/// Finds the Gradle project of an IDE module name. Since Gradle sync, module names are the root
/// project's name, the project path and often a source set, joined with dots; older projects
/// use the bare directory name. The name is matched against directories with a build script,
/// dropping leading parts (the root project's name, which can contain dots itself) until one
/// exists. A trailing source set is only dropped when the name doesn't match as written.
pub(crate) fn resolve_module(root: &Path, module_name: &str) -> Option<GradleModule> {
    let parts = module_name
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    if let Some(module) = match_project(root, &parts, None) {
        return Some(module);
    }
    let (last, rest) = parts.split_last()?;
    if is_source_set(last) {
        if let Some(module) = match_project(root, rest, Some(last)) {
            return Some(module);
        }
        // Only the root project's name before the source set.
        if rest.len() == 1 && has_build_script(root) {
            return Some(GradleModule {
                gradle_path: String::new(),
                dir: root.to_path_buf(),
                source_set: Some((*last).to_owned()),
            });
        }
    }
    (parts.len() == 1 && has_build_script(root)).then(|| GradleModule {
        gradle_path: String::new(),
        dir: root.to_path_buf(),
        source_set: None,
    })
}

fn match_project(root: &Path, parts: &[&str], source_set: Option<&str>) -> Option<GradleModule> {
    (0..parts.len()).find_map(|start| {
        let segments = &parts[start..];
        let dir = segments
            .iter()
            .fold(root.to_path_buf(), |dir, part| dir.join(part));
        has_build_script(&dir).then(|| GradleModule {
            gradle_path: segments.iter().map(|part| format!(":{part}")).collect(),
            dir,
            source_set: source_set.map(str::to_owned),
        })
    })
}

fn is_source_set(part: &str) -> bool {
    matches!(
        part,
        "main" | "test" | "unitTest" | "androidTest" | "testFixtures"
    ) || ((part.ends_with("Main") || part.ends_with("Test")) && part.len() > 4)
}

fn has_build_script(dir: &Path) -> bool {
    ["build.gradle.kts", "build.gradle"]
        .iter()
        .any(|name| dir.join(name).is_file())
}

/// How to start Gradle for the project in `project_dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GradleLauncher {
    /// `./gradlew`, or `gradle` when the project has no wrapper.
    pub(crate) program: &'static str,
    /// Where to run it: the wrapper's directory, else `project_dir`.
    pub(crate) cwd: PathBuf,
    /// `-p <dir>` when `project_dir` isn't `cwd`, relative to `cwd`.
    pub(crate) project_arg: Option<String>,
}

/// Finds the Gradle wrapper in `project_dir` or above it, up to `root`. Running the wrapper
/// with `-p` keeps the IDE's meaning of task names relative to the configuration's project.
pub(crate) fn gradle_launcher(root: &Path, project_dir: &Path) -> GradleLauncher {
    let wrapper_dir = project_dir
        .ancestors()
        .take_while(|dir| dir.starts_with(root))
        .find(|dir| dir.join("gradlew").is_file());
    match wrapper_dir {
        Some(dir) => GradleLauncher {
            program: "./gradlew",
            cwd: dir.to_path_buf(),
            project_arg: project_dir
                .strip_prefix(dir)
                .ok()
                .filter(|relative| !relative.as_os_str().is_empty())
                .map(|relative| relative.to_string_lossy().into_owned()),
        },
        None => GradleLauncher {
            program: "gradle",
            cwd: project_dir.to_path_buf(),
            project_arg: None,
        },
    }
}

#[cfg(test)]
#[path = "module_tests.rs"]
mod tests;
