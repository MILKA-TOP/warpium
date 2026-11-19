//! Reading `docker inspect` (or `podman inspect`) output, choosing the container a session runs in
//! and mapping host paths into bind mounts.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, FixedOffset};
use serde::Deserialize;

use crate::invocation::{ComposeService, ContainerRef};

const COMPOSE_PROJECT_LABEL: &str = "com.docker.compose.project";
const COMPOSE_SERVICE_LABEL: &str = "com.docker.compose.service";
const COMPOSE_ONE_OFF_LABEL: &str = "com.docker.compose.oneoff";
const COMPOSE_WORKING_DIR_LABEL: &str = "com.docker.compose.project.working_dir";
const PODMAN_COMPOSE_PROJECT_LABEL: &str = "io.podman.compose.project";

#[derive(Clone, Debug, PartialEq)]
pub struct ContainerInfo {
    pub id: String,
    /// Without Docker's leading `/`.
    pub name: String,
    /// The image as the container was created from (`node:20`, `docker.io/library/node:20`).
    pub image: String,
    pub image_id: String,
    pub hostname: String,
    pub labels: HashMap<String, String>,
    pub created: Option<DateTime<FixedOffset>>,
    pub running: bool,
    pub mounts: Vec<Mount>,
}

impl ContainerInfo {
    /// The name, or else the short id.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.id[..self.id.len().min(12)]
        } else {
            &self.name
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub bind: bool,
    pub source: String,
    pub destination: String,
    pub writable: bool,
}

#[derive(Deserialize)]
struct RawContainer {
    #[serde(rename = "Id", default)]
    id: String,
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "Created", default)]
    created: String,
    #[serde(rename = "Image", default)]
    image_id: String,
    #[serde(rename = "ImageName", default)]
    image_name: Option<String>,
    #[serde(rename = "Config", default)]
    config: Option<RawConfig>,
    #[serde(rename = "State", default)]
    state: Option<RawState>,
    #[serde(rename = "Mounts", default)]
    mounts: Option<Vec<RawMount>>,
}

#[derive(Deserialize, Default)]
struct RawConfig {
    #[serde(rename = "Image", default)]
    image: String,
    #[serde(rename = "Hostname", default)]
    hostname: String,
    #[serde(rename = "Labels", default)]
    labels: Option<HashMap<String, String>>,
}

#[derive(Deserialize, Default)]
struct RawState {
    #[serde(rename = "Running", default)]
    running: bool,
}

#[derive(Deserialize)]
struct RawMount {
    #[serde(rename = "Type", default)]
    kind: String,
    #[serde(rename = "Source", default)]
    source: String,
    #[serde(rename = "Destination", default)]
    destination: String,
    #[serde(rename = "RW", default)]
    writable: bool,
}

/// Parses the JSON array `inspect` prints for one or more containers.
pub fn parse_inspect(json: &str) -> Result<Vec<ContainerInfo>, String> {
    let raw: Vec<RawContainer> = serde_json::from_str(json.trim())
        .map_err(|err| format!("unexpected inspect output: {err}"))?;
    Ok(raw
        .into_iter()
        .map(|raw| {
            let config = raw.config.unwrap_or_default();
            let image = if config.image.is_empty() {
                raw.image_name.unwrap_or_default()
            } else {
                config.image
            };
            ContainerInfo {
                name: raw.name.trim_start_matches('/').to_owned(),
                id: raw.id,
                image,
                image_id: raw.image_id,
                hostname: config.hostname,
                labels: config.labels.unwrap_or_default(),
                created: DateTime::parse_from_rfc3339(&raw.created).ok(),
                running: raw.state.unwrap_or_default().running,
                mounts: raw
                    .mounts
                    .unwrap_or_default()
                    .into_iter()
                    .map(|mount| Mount {
                        bind: mount.kind == "bind",
                        source: mount.source,
                        destination: mount.destination,
                        writable: mount.writable,
                    })
                    .collect(),
            }
        })
        .collect())
}

/// Normalizes an image reference so `node`, `node:latest`, `docker.io/library/node` and
/// `docker.io/library/node:latest@sha256:…` compare equal.
pub fn normalize_image(image: &str) -> String {
    let image = image.split('@').next().unwrap_or(image);
    let (registry, path) = match image.split_once('/') {
        Some((first, rest))
            if first.contains('.') || first.contains(':') || first == "localhost" =>
        {
            (Some(first), rest)
        }
        _ => (None, image),
    };
    let registry = registry.filter(|registry| {
        !matches!(
            *registry,
            "docker.io" | "index.docker.io" | "registry-1.docker.io" | "localhost"
        )
    });
    let path = match registry {
        Some(_) => path,
        None => path.strip_prefix("library/").unwrap_or(path),
    };
    let last = path.rsplit('/').next().unwrap_or(path);
    let tag = if last.contains(':') { "" } else { ":latest" };
    match registry {
        Some(registry) => format!("{registry}/{path}{tag}"),
        None => format!("{path}{tag}"),
    }
}

fn image_matches(container: &ContainerInfo, wanted: &str) -> bool {
    if normalize_image(&container.image) == normalize_image(wanted) {
        return true;
    }
    let wanted_id = wanted.strip_prefix("sha256:").unwrap_or(wanted);
    let image_id = container
        .image_id
        .strip_prefix("sha256:")
        .unwrap_or(&container.image_id);
    wanted_id.len() >= 12
        && wanted_id.chars().all(|c| c.is_ascii_hexdigit())
        && image_id.starts_with(wanted_id)
}

/// Compose's normalization of a project name.
fn normalize_project(project: &str) -> String {
    project
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect()
}

/// What is known about where the session was started.
#[derive(Clone, Copy, Debug, Default)]
pub struct SelectionHints<'a> {
    /// The host directory the command was run in.
    pub cwd: Option<&'a Path>,
    /// The session's hostname as reported by its shell, when Warp tracks the shell inside the
    /// container: Docker's default hostname is the short container id.
    pub hostname: Option<&'a str>,
}

fn hostname_matches(container: &ContainerInfo, hostname: Option<&str>) -> bool {
    hostname.is_some_and(|hostname| {
        !hostname.is_empty()
            && (container.hostname == hostname
                || (hostname.len() >= 12 && container.id.starts_with(hostname)))
    })
}

/// Resolves `.` and `..` without touching the file system.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}

/// The directory Compose takes as the project's working directory, when it can be told from the
/// command line.
fn compose_project_dir(service: &ComposeService, cwd: Option<&Path>) -> Option<PathBuf> {
    let resolve = |path: &str| {
        let path = Path::new(path);
        if path.is_absolute() {
            Some(lexical_normalize(path))
        } else {
            cwd.map(|cwd| lexical_normalize(&cwd.join(path)))
        }
    };
    if let Some(directory) = &service.project_directory {
        return resolve(directory);
    }
    if let Some(file) = service.files.first() {
        return resolve(file).and_then(|file| file.parent().map(Path::to_path_buf));
    }
    cwd.map(Path::to_path_buf)
}

/// How well a container's Compose working directory matches: 2 for the project directory itself,
/// 1 for a parent of the shell's directory (Compose looks for the file upwards), 0 otherwise.
fn compose_dir_score(
    container: &ContainerInfo,
    service: &ComposeService,
    hints: &SelectionHints<'_>,
) -> u8 {
    let Some(working_dir) = container.labels.get(COMPOSE_WORKING_DIR_LABEL) else {
        return 0;
    };
    let working_dir = Path::new(working_dir);
    if compose_project_dir(service, hints.cwd).is_some_and(|dir| dir == working_dir) {
        return 2;
    }
    let explicit = service.project_directory.is_some() || !service.files.is_empty();
    if !explicit && hints.cwd.is_some_and(|cwd| cwd.starts_with(working_dir)) {
        return 1;
    }
    0
}

fn compose_project_matches(container: &ContainerInfo, project: Option<&str>) -> bool {
    let Some(project) = project else {
        return true;
    };
    let project = normalize_project(project);
    [COMPOSE_PROJECT_LABEL, PODMAN_COMPOSE_PROJECT_LABEL]
        .iter()
        .any(|label| container.labels.get(*label) == Some(&project))
}

/// Picks the running container `target` refers to; the newest one when several match equally.
pub fn select_container<'a>(
    containers: &'a [ContainerInfo],
    target: &ContainerRef,
    hints: &SelectionHints<'_>,
) -> Option<&'a ContainerInfo> {
    let running = containers.iter().filter(|container| container.running);
    match target {
        ContainerRef::Named(name) => running
            .filter(|container| {
                container.name == *name
                    || (name.len() >= 4
                        && name.chars().all(|c| c.is_ascii_hexdigit())
                        && container.id.starts_with(name.as_str()))
            })
            .max_by_key(|container| container.created),
        ContainerRef::Image(image) => running
            .filter(|container| image_matches(container, image))
            .max_by_key(|container| {
                (
                    hostname_matches(container, hints.hostname),
                    container.created,
                )
            }),
        ContainerRef::ComposeService(service) => running
            .filter(|container| {
                container.labels.get(COMPOSE_SERVICE_LABEL) == Some(&service.service)
                    && compose_project_matches(container, service.project.as_deref())
            })
            .max_by_key(|container| {
                let one_off = container
                    .labels
                    .get(COMPOSE_ONE_OFF_LABEL)
                    .is_some_and(|value| value.eq_ignore_ascii_case("true"));
                (
                    hostname_matches(container, hints.hostname),
                    compose_dir_score(container, service, hints),
                    one_off == service.one_off,
                    container.created,
                )
            }),
    }
}

/// The writable bind mount to place pasted files in: the innermost one holding `cwd`, so files
/// land in the project the session works on rather than in an unrelated mount.
pub fn mount_for_cwd<'a>(mounts: &'a [Mount], cwd: &Path) -> Option<&'a Mount> {
    mounts
        .iter()
        .filter(|mount| {
            mount.bind
                && mount.writable
                && Path::new(&mount.source).is_absolute()
                && mount.destination.starts_with('/')
                && cwd.starts_with(&mount.source)
        })
        .max_by_key(|mount| mount.source.len())
}

/// The path inside the container of `host_path`, a file under `mount`'s source.
pub fn container_path(mount: &Mount, host_path: &Path) -> Option<String> {
    let relative = host_path.strip_prefix(&mount.source).ok()?;
    let mut path = mount.destination.trim_end_matches('/').to_owned();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return None;
        };
        path.push('/');
        path.push_str(part.to_str()?);
    }
    Some(path)
}

#[cfg(test)]
#[path = "inspect_tests.rs"]
mod tests;
