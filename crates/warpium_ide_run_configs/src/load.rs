//! Finding and reading a project's run configuration files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use roxmltree::{Document, Node};

use crate::mapping::{Context, Mapped, map_configuration};
use crate::model::{ConfigOrigin, IdeRun, IdeRunConfig, IdeRunConfigs, RunStep};
use crate::xml::{children, line_of};

/// The folder of shared run configurations next to the project's sources.
pub const RUN_DIR: &str = ".run";
pub const IDEA_DIR: &str = ".idea";
/// The folder of shared run configurations inside `.idea`.
pub const RUN_CONFIGURATIONS_DIR: &str = ".idea/runConfigurations";
/// The IDE's per-user project state, with the personal run configurations.
pub const WORKSPACE_FILE: &str = ".idea/workspace.xml";

/// Run configuration files larger than this are skipped; real ones are a few kilobytes and a
/// busy `workspace.xml` a few hundred.
const MAX_FILE_SIZE: u64 = 8 * 1024 * 1024;
/// Compound configurations nested deeper than this are not followed.
const MAX_COMPOUND_DEPTH: usize = 8;

/// The directories to watch for changes to the project's run configurations, whether or not
/// they exist yet.
pub fn watched_dirs(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join(RUN_DIR),
        root.join(IDEA_DIR),
        root.join(RUN_CONFIGURATIONS_DIR),
    ]
}

/// Whether a change to `path` can change the run configurations of the project at `root`.
pub fn is_ide_config_path(root: &Path, path: &Path) -> bool {
    path == root.join(IDEA_DIR)
        || path == root.join(WORKSPACE_FILE)
        || path.starts_with(root.join(RUN_DIR))
        || path.starts_with(root.join(RUN_CONFIGURATIONS_DIR))
}

/// One configuration as read from a file.
struct Parsed {
    name: String,
    type_id: String,
    origin: ConfigOrigin,
    temporary: bool,
    mapped: Mapped,
}

impl Parsed {
    fn key(&self) -> String {
        IdeRunConfig::key_for(&self.type_id, &self.name)
    }
}

/// Reads the run configurations of the project at `root`. Files and configurations that can't
/// be read are skipped with a warning.
pub fn load_project(root: &Path, home: Option<&Path>) -> IdeRunConfigs {
    let ctx = Context { root, home };
    let mut parsed = Vec::new();
    let mut warnings = Vec::new();
    for relative in shared_files(root) {
        match read(&root.join(&relative)) {
            Ok(text) => {
                let origin = ConfigOrigin::Shared(relative.clone());
                match parse_document(&text, &origin, ctx) {
                    Ok((configs, file_warnings)) => {
                        parsed.extend(configs);
                        warnings.extend(
                            file_warnings
                                .into_iter()
                                .map(|w| format!("{relative}: {w}")),
                        );
                    }
                    Err(message) => warnings.push(format!("{relative}: {message}")),
                }
            }
            Err(message) => warnings.push(format!("{relative}: {message}")),
        }
    }
    let mut selected = None;
    let workspace = root.join(WORKSPACE_FILE);
    if workspace.is_file() {
        // A committed workspace.xml comes from the repository like the shared files, so its
        // commands need the same trust.
        let origin = if is_tracked_by_git(root, &workspace) {
            ConfigOrigin::Shared(WORKSPACE_FILE.to_owned())
        } else {
            ConfigOrigin::Personal
        };
        match read(&workspace).and_then(|text| {
            let document = Document::parse(&text).map_err(|err| format!("invalid XML: {err}"))?;
            let run_manager = document.root_element().children().find(|node| {
                node.has_tag_name("component") && node.attribute("name") == Some("RunManager")
            });
            Ok(match run_manager {
                Some(component) => {
                    selected = component.attribute("selected").map(str::to_owned);
                    parse_component(component, &origin, ctx)
                }
                None => (Vec::new(), Vec::new()),
            })
        }) {
            Ok((configs, file_warnings)) => {
                parsed.extend(configs);
                warnings.extend(
                    file_warnings
                        .into_iter()
                        .map(|warning| format!("{WORKSPACE_FILE}: {warning}")),
                );
            }
            Err(message) => warnings.push(format!("{WORKSPACE_FILE}: {message}")),
        }
    }
    let mut configs = resolve(parsed, selected.as_deref());
    configs.warnings.extend(warnings);
    configs
}

/// Combines the configurations read from all files: drops duplicates (the first one wins),
/// leaves out temporary configurations unless there is nothing else, resolves compound members
/// and picks the IDE's selection (`RunManager`'s `selected`, such as `Gradle.detekt`) as the
/// default.
fn resolve(parsed: Vec<Parsed>, selected: Option<&str>) -> IdeRunConfigs {
    let mut seen = HashSet::new();
    let parsed = parsed
        .into_iter()
        .filter(|config| seen.insert(config.key()))
        .collect::<Vec<_>>();
    let has_permanent = parsed.iter().any(|config| !config.temporary);
    let configs = parsed
        .iter()
        .filter(|config| !has_permanent || !config.temporary)
        .map(|config| {
            let key = config.key();
            let run = match steps(&parsed, &key, 0) {
                Ok(steps) => IdeRun::Steps(steps),
                Err(reason) => IdeRun::Unsupported(reason),
            };
            IdeRunConfig {
                key,
                name: config.name.clone(),
                type_id: config.type_id.clone(),
                origin: config.origin.clone(),
                run,
            }
        })
        .collect::<Vec<_>>();
    let default_key = selected.and_then(|selected| {
        configs
            .iter()
            .filter(|config| config.steps().is_some())
            .filter(|config| {
                selected
                    .strip_suffix(config.name.as_str())
                    .and_then(|prefix| prefix.strip_suffix('.'))
                    .is_some_and(|prefix| !prefix.is_empty())
            })
            .max_by_key(|config| config.name.len())
            .map(|config| config.key.clone())
    });
    IdeRunConfigs {
        configs,
        default_key,
        warnings: Vec::new(),
    }
}

/// The steps of the configuration `key`, following compound members.
fn steps(parsed: &[Parsed], key: &str, depth: usize) -> Result<Vec<RunStep>, String> {
    let Some(config) = parsed.iter().find(|config| config.key() == key) else {
        return Err("the configuration wasn't found".to_owned());
    };
    match &config.mapped {
        Mapped::Step(action) => Ok(vec![RunStep {
            config_key: key.to_owned(),
            label: config.name.clone(),
            shared: config.origin.is_shared(),
            action: action.clone(),
        }]),
        Mapped::Unsupported(reason) => Err(reason.clone()),
        Mapped::Compound(_) if depth >= MAX_COMPOUND_DEPTH => {
            Err("compound configurations are nested too deeply".to_owned())
        }
        Mapped::Compound(members) if members.is_empty() => {
            Err("the compound configuration has no members".to_owned())
        }
        Mapped::Compound(members) => {
            let mut all = Vec::new();
            for (member_type, member_name) in members {
                let member_key = IdeRunConfig::key_for(member_type, member_name);
                if !parsed.iter().any(|member| member.key() == member_key) {
                    return Err(format!("its member \"{member_name}\" wasn't found"));
                }
                all.extend(
                    steps(parsed, &member_key, depth + 1)
                        .map_err(|reason| format!("its member \"{member_name}\": {reason}"))?,
                );
            }
            Ok(all)
        }
    }
}

/// The shared run configuration files of the project, relative to `root`, sorted.
fn shared_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for (dir, suffix) in [(RUN_DIR, ".run.xml"), (RUN_CONFIGURATIONS_DIR, ".xml")] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        let mut names = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(suffix) && !name.starts_with('.'))
            .collect::<Vec<_>>();
        names.sort();
        files.extend(names.into_iter().map(|name| format!("{dir}/{name}")));
    }
    files
}

fn read(path: &Path) -> Result<String, String> {
    let size = std::fs::metadata(path)
        .map_err(|err| format!("couldn't read the file: {err}"))?
        .len();
    if size > MAX_FILE_SIZE {
        return Err("the file is too large".to_owned());
    }
    std::fs::read_to_string(path).map_err(|err| format!("couldn't read the file: {err}"))
}

/// The configurations of a shared file: a `ProjectRunConfigurationManager` component, or (in
/// older files) a bare `<configuration>`.
fn parse_document(
    text: &str,
    origin: &ConfigOrigin,
    ctx: Context<'_>,
) -> Result<(Vec<Parsed>, Vec<String>), String> {
    let document = Document::parse(text).map_err(|err| format!("invalid XML: {err}"))?;
    let root = document.root_element();
    if root.has_tag_name("configuration") {
        let mut warnings = Vec::new();
        let configs = parse_configuration(root, origin, ctx, &mut warnings)
            .into_iter()
            .collect();
        return Ok((configs, warnings));
    }
    Ok(parse_component(root, origin, ctx))
}

fn parse_component(
    component: Node<'_, '_>,
    origin: &ConfigOrigin,
    ctx: Context<'_>,
) -> (Vec<Parsed>, Vec<String>) {
    let mut warnings = Vec::new();
    let configs = children(component, "configuration")
        .filter_map(|node| parse_configuration(node, origin, ctx, &mut warnings))
        .collect();
    (configs, warnings)
}

fn parse_configuration(
    node: Node<'_, '_>,
    origin: &ConfigOrigin,
    ctx: Context<'_>,
    warnings: &mut Vec<String>,
) -> Option<Parsed> {
    // Templates for new configurations of a type, not configurations.
    if node.attribute("default") == Some("true") {
        return None;
    }
    let (Some(name), Some(type_id)) = (
        node.attribute("name").filter(|name| !name.is_empty()),
        node.attribute("type").filter(|type_id| !type_id.is_empty()),
    ) else {
        warnings.push(format!(
            "line {}: a run configuration without a name or type was skipped",
            line_of(node)
        ));
        return None;
    };
    Some(Parsed {
        name: name.to_owned(),
        type_id: type_id.to_owned(),
        origin: origin.clone(),
        temporary: node.attribute("temporary") == Some("true"),
        mapped: map_configuration(node, type_id, ctx),
    })
}

/// Whether `file` is in the git index of the repository holding `root`. A heuristic that reads
/// the index for the file's path rather than running git: entries store the full path followed
/// by a NUL in index versions 2 and 3 (version 4 compresses paths, so a tracked file can be
/// missed there).
fn is_tracked_by_git(root: &Path, file: &Path) -> bool {
    let Some((work_tree, git_dir)) = root.ancestors().find_map(|dir| {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some((dir.to_path_buf(), dot_git));
        }
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let git_dir = pointer.trim().strip_prefix("gitdir:")?.trim();
        Some((dir.to_path_buf(), dir.join(git_dir)))
    }) else {
        return false;
    };
    let Ok(relative) = file.strip_prefix(&work_tree) else {
        return false;
    };
    let Ok(index) = std::fs::read(git_dir.join("index")) else {
        return false;
    };
    let mut needle = relative.to_string_lossy().replace('\\', "/").into_bytes();
    needle.push(0);
    index.windows(needle.len()).any(|window| window == needle)
}

#[cfg(test)]
#[path = "load_tests.rs"]
mod tests;
