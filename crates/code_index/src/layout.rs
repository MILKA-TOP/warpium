//! Gradle project layout conventions, read from paths alone (no Gradle model).

use crate::model::{Language, Origin};

/// Directory names never descended into by the main walk. Generated sources under
/// `build/generated` are walked separately.
pub(crate) const SKIPPED_DIRS: &[&str] = &["build", "node_modules", "out", "target"];

/// Generated Android resource classes. They are huge and better resolved to their XML sources
/// (phase 2), so they are not indexed.
const SKIPPED_GENERATED_FILES: &[&str] = &["R.java", "BR.java", "Manifest.java"];

/// Where a file sits in the project.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Placement {
    pub module: String,
    pub source_set: String,
    pub origin: Origin,
}

/// Whether `rel_path` (relative to the index root, `/`-separated) is a source file this index
/// covers: a Kotlin or Java file outside hidden and build directories, or a generated source
/// under `build/generated/`.
pub(crate) fn is_indexable(rel_path: &str) -> bool {
    if Language::from_path(std::path::Path::new(rel_path)).is_none() {
        return false;
    }
    let components: Vec<&str> = rel_path.split('/').collect();
    let Some((file_name, dirs)) = components.split_last() else {
        return false;
    };
    if let Some(generated) = generated_index(dirs) {
        return !SKIPPED_GENERATED_FILES.contains(file_name)
            && dirs[..generated - 1].iter().all(|dir| is_walkable_dir(dir));
    }
    dirs.iter().all(|dir| is_walkable_dir(dir))
}

pub(crate) fn is_walkable_dir(name: &str) -> bool {
    !name.starts_with('.') && !SKIPPED_DIRS.contains(&name)
}

/// Index of the `generated` component in a `build/generated/…` path.
fn generated_index(dirs: &[&str]) -> Option<usize> {
    dirs.windows(2)
        .position(|pair| pair[0] == "build" && pair[1] == "generated")
        .map(|build| build + 1)
}

/// Classifies `rel_path` by Gradle conventions: the module is the directory before `src/` (or
/// `build/`), the source set is the directory after `src/`, and generated code lives under
/// `build/generated/`.
pub(crate) fn place(rel_path: &str) -> Placement {
    let components: Vec<&str> = rel_path.split('/').collect();
    let dirs = &components[..components.len().saturating_sub(1)];

    if let Some(generated) = generated_index(dirs) {
        let module = dirs[..generated - 1].join("/");
        // `ksp/<set>/kotlin`, `ksp/metadata/commonMain/kotlin`, `source/buildConfig/<set>`, …
        let source_set = dirs[generated + 1..]
            .iter()
            .find(|dir| is_source_set_name(dir))
            .map(|dir| (*dir).to_owned())
            .unwrap_or_default();
        return Placement {
            module,
            source_set,
            origin: Origin::Generated,
        };
    }

    match dirs.iter().position(|dir| *dir == "src") {
        Some(src) => {
            let source_set = dirs.get(src + 1).copied().unwrap_or_default();
            Placement {
                module: dirs[..src].join("/"),
                source_set: source_set.to_owned(),
                origin: if is_test_source_set(source_set) {
                    Origin::Test
                } else {
                    Origin::Main
                },
            }
        }
        None => Placement::default(),
    }
}

fn is_source_set_name(name: &str) -> bool {
    matches!(name, "main" | "test" | "debug" | "release")
        || name.ends_with("Main")
        || name.ends_with("Test")
        || name.ends_with("Debug")
        || name.ends_with("Release")
}

/// `test`, `androidTest`, `testFixtures`, `commonTest`, `jvmTest`, `androidUnitTest`, …
fn is_test_source_set(name: &str) -> bool {
    name.starts_with("test") || name.ends_with("Test") || name == "androidTest"
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
