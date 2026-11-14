use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::fs_util::{atomic_write, backup_existing, file_mode};
use crate::manifest::Manifest;
use crate::report::ImportReport;
use crate::source::WarpInstallation;
use crate::sync::{SyncDecision, content_hash, decide};
use crate::{ImportOptions, WarpiumTarget};

/// Files larger than this are not imported; Warp's config files are a few KB.
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// Upper bound on files imported per category, so a stray large tree can't stall startup.
const MAX_FILES_PER_CATEGORY: usize = 5_000;
const MAX_DEPTH: usize = 16;
/// Entry names never imported: macOS metadata, VCS data and dependency trees inside skills.
const IGNORED_NAMES: &[&str] = &[".DS_Store", ".git", "node_modules"];

/// An extra check on a file's content before it is imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileGuard {
    None,
    /// Skip the file if it looks like it holds credentials inline (MCP server `env`/`headers`,
    /// tokens, API keys).
    NoInlineSecrets,
}

/// A file or directory of Warp configuration that is copied as-is into Warpium.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCategory {
    /// Manifest and report prefix, e.g. `themes`.
    pub key: &'static str,
    pub source: PathBuf,
    pub target: PathBuf,
    pub is_dir: bool,
    pub guard: FileGuard,
}

/// The file categories imported from `source` into `target`.
pub fn file_categories(source: &WarpInstallation, target: &WarpiumTarget) -> Vec<FileCategory> {
    let data_dir = |key: &'static str| FileCategory {
        key,
        source: source.data_dir.join(key),
        target: target.data_dir.join(key),
        is_dir: true,
        guard: FileGuard::None,
    };
    vec![
        data_dir("themes"),
        data_dir("workflows"),
        data_dir("launch_configurations"),
        data_dir("tab_configs"),
        data_dir("default_tab_configs"),
        data_dir("custom_model_routers"),
        FileCategory {
            key: "skills",
            source: source.home_config_dir.join("skills"),
            target: target.home_config_dir.join("skills"),
            is_dir: true,
            guard: FileGuard::None,
        },
        FileCategory {
            key: "keybindings.yaml",
            source: source.data_dir.join("keybindings.yaml"),
            target: target.keybindings_file.clone(),
            is_dir: false,
            guard: FileGuard::None,
        },
        FileCategory {
            key: ".mcp.json",
            source: source.home_config_dir.join(".mcp.json"),
            target: target.home_config_dir.join(".mcp.json"),
            is_dir: false,
            guard: FileGuard::NoInlineSecrets,
        },
    ]
}

/// Runs the 3-way sync for every file category, updating `manifest.files` and `report`.
pub fn sync_file_categories(
    categories: &[FileCategory],
    target: &WarpiumTarget,
    manifest: &mut Manifest,
    initial_import: bool,
    options: &ImportOptions,
    report: &mut ImportReport,
) {
    let backup_root = target.backup_dir(&options.run_id);
    for category in categories {
        sync_category(
            category,
            &backup_root,
            manifest,
            initial_import,
            options,
            report,
        );
    }
}

fn sync_category(
    category: &FileCategory,
    backup_root: &Path,
    manifest: &mut Manifest,
    initial_import: bool,
    options: &ImportOptions,
    report: &mut ImportReport,
) {
    // Relative path within the category -> absolute source path.
    let mut source_files = BTreeMap::new();
    if category.is_dir {
        let mut truncated = false;
        walk(&category.source, "", 0, &mut source_files, &mut truncated);
        if truncated {
            report.skipped.push((
                category.key.to_owned(),
                format!("more than {MAX_FILES_PER_CATEGORY} files; the rest was not imported"),
            ));
        }
    } else if category.source.is_file() {
        source_files.insert(String::new(), category.source.clone());
    }

    let key_for = |relative: &str| {
        if relative.is_empty() {
            category.key.to_owned()
        } else {
            format!("{}/{relative}", category.key)
        }
    };
    let prefix = format!("{}/", category.key);
    let mut relatives: BTreeSet<String> = source_files.keys().cloned().collect();
    relatives.extend(manifest.files.keys().filter_map(|key| {
        if category.is_dir {
            key.strip_prefix(&prefix).map(str::to_owned)
        } else {
            (key == category.key).then(String::new)
        }
    }));

    for relative in relatives {
        let key = key_for(&relative);
        let target_path = if relative.is_empty() {
            category.target.clone()
        } else {
            category.target.join(&relative)
        };
        let source_path = source_files.get(&relative);
        let source_bytes = match source_path.map(|path| read_source(path, category.guard)) {
            None => None,
            Some(Ok(bytes)) => Some(bytes),
            Some(Err(SourceSkip::Skip(reason))) => {
                report.skipped.push((key, reason));
                continue;
            }
            Some(Err(SourceSkip::Error(err))) => {
                report.errors.push((key, err));
                continue;
            }
        };
        let target_hash = std::fs::read(&target_path)
            .ok()
            .map(|bytes| content_hash(&bytes));
        let source_hash = source_bytes.as_deref().map(content_hash);
        let base = manifest.files.get(&key).cloned();
        let decision = decide(
            base.as_deref(),
            source_hash.as_deref(),
            target_hash.as_deref(),
            initial_import,
        );
        log::debug!("[Warp import] file {key}: {decision:?}");

        match decision {
            SyncDecision::Create | SyncDecision::Update | SyncDecision::Overwrite => {
                let (Some(bytes), Some(hash), Some(source_path)) =
                    (source_bytes, source_hash, source_path)
                else {
                    continue;
                };
                if !options.dry_run
                    && let Err(err) =
                        write_with_backup(&target_path, &bytes, source_path, backup_root, &key)
                {
                    report.errors.push((key, err.to_string()));
                    continue;
                }
                manifest.files.insert(key.clone(), hash);
                if decision == SyncDecision::Create {
                    report.imported.push(key);
                } else {
                    report.updated.push(key);
                }
            }
            SyncDecision::InSync => {
                if let Some(hash) = source_hash {
                    manifest.files.insert(key, hash);
                }
                report.unchanged += 1;
            }
            SyncDecision::Remove => {
                manifest.files.remove(&key);
                report
                    .skipped
                    .push((key, "removed in Warp; kept in Warpium".to_owned()));
            }
            SyncDecision::KeepLocal => {
                if source_hash.is_none() {
                    manifest.files.remove(&key);
                }
                report.unchanged += 1;
            }
            SyncDecision::Conflict => report.conflicts.push(key),
        }
    }
}

enum SourceSkip {
    /// Deliberately not imported.
    Skip(String),
    Error(String),
}

fn read_source(path: &Path, guard: FileGuard) -> Result<Vec<u8>, SourceSkip> {
    let len = path
        .metadata()
        .map_err(|err| SourceSkip::Error(err.to_string()))?
        .len();
    if len > MAX_FILE_BYTES {
        return Err(SourceSkip::Skip(format!(
            "larger than {} MB",
            MAX_FILE_BYTES / 1024 / 1024
        )));
    }
    let bytes = std::fs::read(path).map_err(|err| SourceSkip::Error(err.to_string()))?;
    match guard {
        FileGuard::None => Ok(bytes),
        FileGuard::NoInlineSecrets if json_has_inline_secrets(&bytes) => Err(SourceSkip::Skip(
            "may contain credentials (env, headers or tokens); copy it by hand if wanted"
                .to_owned(),
        )),
        FileGuard::NoInlineSecrets => Ok(bytes),
    }
}

fn write_with_backup(
    target_path: &Path,
    bytes: &[u8],
    source_path: &Path,
    backup_root: &Path,
    key: &str,
) -> std::io::Result<()> {
    backup_existing(target_path, backup_root, Path::new(key))?;
    atomic_write(target_path, bytes, file_mode(source_path))
}

/// Collects regular files under `dir` (following symlinks, as Preview's config directory links
/// into Stable's) into `out`, keyed by `/`-separated path relative to the category root.
fn walk(
    dir: &Path,
    relative: &str,
    depth: usize,
    out: &mut BTreeMap<String, PathBuf>,
    truncated: &mut bool,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if IGNORED_NAMES.contains(&name.as_str()) || name.starts_with("._") {
            continue;
        }
        let path = entry.path();
        let child_relative = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        // `metadata` follows symlinks; broken links are skipped.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            walk(&path, &child_relative, depth + 1, out, truncated);
        } else if meta.is_file() {
            if out.len() >= MAX_FILES_PER_CATEGORY {
                *truncated = true;
                return;
            }
            out.insert(child_relative, path);
        }
    }
}

/// Heuristic for JSON config that embeds credentials. Unparseable content counts as secret, so
/// the check fails closed.
pub(crate) fn json_has_inline_secrets(bytes: &[u8]) -> bool {
    fn is_secret_key(key: &str) -> bool {
        let key = key.to_ascii_lowercase();
        [
            "token",
            "secret",
            "password",
            "apikey",
            "api_key",
            "api-key",
            "authorization",
        ]
        .iter()
        .any(|needle| key.contains(needle))
    }
    fn visit(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
                let non_empty = match value {
                    serde_json::Value::Object(inner) => !inner.is_empty(),
                    serde_json::Value::String(text) => !text.is_empty(),
                    serde_json::Value::Array(items) => !items.is_empty(),
                    serde_json::Value::Null
                    | serde_json::Value::Bool(_)
                    | serde_json::Value::Number(_) => false,
                };
                let lower = key.to_ascii_lowercase();
                (non_empty && (lower == "env" || lower == "headers" || is_secret_key(key)))
                    || visit(value)
            }),
            serde_json::Value::Array(items) => items.iter().any(visit),
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => false,
        }
    }
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(value) => visit(&value),
        Err(_) => true,
    }
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
