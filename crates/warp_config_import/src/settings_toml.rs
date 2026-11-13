use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

use crate::WarpiumTarget;
use crate::report::ImportReport;
use crate::source::WarpInstallation;
use crate::sync::{SyncDecision, content_hash, decide};

/// `settings.toml` paths that are never imported (a path and everything below it):
/// Warpium's own settings, and launch-at-login, which would register both apps with the OS.
pub const SETTINGS_DENYLIST: &[&str] = &["warpium", "general.login_item"];

/// Why the settings file couldn't be merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsMergeError {
    /// Warp's `settings.toml` has a syntax error.
    SourceUnparseable(String),
    /// Warpium's `settings.toml` has a syntax error; it is left alone so the user can fix it.
    TargetUnparseable(String),
}

impl std::fmt::Display for SettingsMergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsMergeError::SourceUnparseable(err) => {
                write!(f, "Warp's settings.toml could not be parsed: {err}")
            }
            SettingsMergeError::TargetUnparseable(err) => {
                write!(f, "Warpium's settings.toml could not be parsed: {err}")
            }
        }
    }
}

/// The result of merging Warp's `settings.toml` into Warpium's.
#[derive(Debug, Clone)]
pub struct SettingsMerge {
    /// New contents for Warpium's `settings.toml`, or `None` when nothing changes.
    pub merged: Option<String>,
    /// Content hash of the Warpium file the merge was computed from (`None` if it was absent), so
    /// the caller can detect that the file changed before the merge is written.
    pub target_snapshot: Option<String>,
    /// The manifest's settings map after this merge is written.
    pub base: BTreeMap<String, String>,
    /// What the merge does, to be added to the run's report once it is written.
    pub report: ImportReport,
}

/// Merges Warp's `settings.toml` (`source`) into Warpium's (`target`, `None` if absent), one
/// setting at a time, with the 3-way rule against `base`.
///
/// A unit of the merge is a whole setting value: each path in `known_setting_paths` (dotted TOML
/// paths of the settings Warpium registers) is merged as one value even when it is a table, so
/// struct- and enum-shaped settings never mix fields from both sides. Anything else in Warp's file
/// (for example settings from a newer Warp) is merged leaf by leaf. Paths in Warp's values that
/// point into Warp's config directories are rewritten through `rewrites`.
pub fn merge_settings_toml(
    source: &str,
    target: Option<&str>,
    known_setting_paths: &[String],
    rewrites: &[(String, String)],
    base: &BTreeMap<String, String>,
    initial_import: bool,
) -> Result<SettingsMerge, SettingsMergeError> {
    let source_doc = source
        .parse::<DocumentMut>()
        .map_err(|err| SettingsMergeError::SourceUnparseable(err.to_string()))?;
    let mut target_doc = match target {
        Some(contents) => contents
            .parse::<DocumentMut>()
            .map_err(|err| SettingsMergeError::TargetUnparseable(err.to_string()))?,
        None => DocumentMut::new(),
    };

    let known: HashSet<&str> = known_setting_paths.iter().map(String::as_str).collect();
    let mut source_units = BTreeMap::new();
    collect_units(
        source_doc.as_table(),
        &mut Vec::new(),
        &known,
        &mut source_units,
    );

    let mut paths: BTreeSet<String> = source_units.keys().cloned().collect();
    paths.extend(base.keys().cloned());

    let mut new_base = base.clone();
    let mut report = ImportReport::default();
    let mut changed = false;
    for path in paths {
        if is_denied(&path) {
            continue;
        }
        let segments: Vec<&str> = path.split('.').collect();
        let source_item = source_units.get(&path).map(|item| {
            let mut item = item.clone();
            rewrite_item(&mut item, rewrites);
            item
        });
        let source_hash = source_item.as_ref().map(item_hash);
        let target_hash = get_path(target_doc.as_table(), &segments).map(item_hash);
        let decision = decide(
            base.get(&path).map(String::as_str),
            source_hash.as_deref(),
            target_hash.as_deref(),
            initial_import,
        );
        log::debug!("[Warp import] setting {path}: {decision:?}");

        match decision {
            SyncDecision::Create | SyncDecision::Update | SyncDecision::Overwrite => {
                let (Some(mut item), Some(hash)) = (source_item, source_hash) else {
                    continue;
                };
                clear_positions(&mut item);
                if set_path(target_doc.as_table_mut(), &segments, item) {
                    changed = true;
                    new_base.insert(path.clone(), hash);
                    if decision == SyncDecision::Create {
                        report.imported.push(path);
                    } else {
                        report.updated.push(path);
                    }
                } else {
                    report
                        .errors
                        .push((path, "could not be placed in settings.toml".to_owned()));
                }
            }
            SyncDecision::Remove => {
                remove_path(target_doc.as_table_mut(), &segments);
                changed = true;
                new_base.remove(&path);
                report.removed.push(path);
            }
            SyncDecision::InSync => {
                if let Some(hash) = source_hash {
                    new_base.insert(path, hash);
                }
                report.unchanged += 1;
            }
            SyncDecision::KeepLocal => {
                if source_hash.is_none() {
                    new_base.remove(&path);
                }
                report.unchanged += 1;
            }
            SyncDecision::Conflict => report.conflicts.push(path),
        }
    }

    Ok(SettingsMerge {
        merged: changed.then(|| target_doc.to_string()),
        target_snapshot: target.map(|contents| content_hash(contents.as_bytes())),
        base: new_base,
        report,
    })
}

/// Prefix rewrites that map paths inside Warp's config directories to Warpium's, both absolute
/// and `~/`-relative.
pub fn path_rewrites(
    source: &WarpInstallation,
    target: &WarpiumTarget,
    home: &Path,
) -> Vec<(String, String)> {
    let mut pairs = vec![(&source.data_dir, &target.data_dir)];
    if source.home_config_dir != source.data_dir {
        pairs.push((&source.home_config_dir, &target.home_config_dir));
    }
    let mut rewrites = Vec::new();
    for (from, to) in pairs {
        rewrites.push((from.display().to_string(), to.display().to_string()));
        if let (Ok(from), Ok(to)) = (from.strip_prefix(home), to.strip_prefix(home)) {
            rewrites.push((
                format!("~/{}", from.display()),
                format!("~/{}", to.display()),
            ));
        }
    }
    rewrites
}

fn is_denied(path: &str) -> bool {
    SETTINGS_DENYLIST.iter().any(|denied| {
        path == *denied
            || path
                .strip_prefix(denied)
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

/// Collects the merge units of `table` into `out`, keyed by dotted path.
fn collect_units(
    table: &dyn TableLike,
    prefix: &mut Vec<String>,
    known: &HashSet<&str>,
    out: &mut BTreeMap<String, Item>,
) {
    for (key, item) in table.iter() {
        prefix.push(key.to_owned());
        let path = prefix.join(".");
        match item.as_table_like() {
            Some(child) if !known.contains(path.as_str()) => {
                collect_units(child, prefix, known, out)
            }
            _ => {
                out.insert(path, item.clone());
            }
        }
        prefix.pop();
    }
}

fn get_path<'a>(table: &'a dyn TableLike, segments: &[&str]) -> Option<&'a Item> {
    match segments {
        [] => None,
        [last] => table.get(last),
        [first, rest @ ..] => get_path(table.get(first)?.as_table_like()?, rest),
    }
}

/// Puts `item` at `segments`, creating intermediate tables. Returns whether it was placed.
fn set_path(table: &mut dyn TableLike, segments: &[&str], item: Item) -> bool {
    match segments {
        [] => false,
        [last] => {
            if item.is_none() {
                return false;
            }
            table.insert(last, item);
            true
        }
        [first, rest @ ..] => {
            if !table.get(first).is_some_and(Item::is_table_like) {
                let mut child = Table::new();
                child.set_implicit(true);
                table.insert(first, Item::Table(child));
            }
            match table.get_mut(first).and_then(Item::as_table_like_mut) {
                Some(child) => set_path(child, rest, item),
                None => false,
            }
        }
    }
}

/// Removes the item at `segments`, then any tables that became empty on the way up.
fn remove_path(table: &mut dyn TableLike, segments: &[&str]) {
    match segments {
        [] => {}
        [last] => {
            table.remove(last);
        }
        [first, rest @ ..] => {
            let now_empty = match table.get_mut(first).and_then(Item::as_table_like_mut) {
                Some(child) => {
                    remove_path(child, rest);
                    child.is_empty()
                }
                None => false,
            };
            if now_empty {
                table.remove(first);
            }
        }
    }
}

/// Inserted tables keep their position from Warp's file, which would interleave them oddly with
/// Warpium's tables. Without a position they are written next to their parent.
fn clear_positions(item: &mut Item) {
    match item {
        Item::Table(table) => {
            table.set_position(None);
            for (_, child) in table.iter_mut() {
                clear_positions(child);
            }
        }
        Item::ArrayOfTables(array) => {
            for table in array.iter_mut() {
                table.set_position(None);
                for (_, child) in table.iter_mut() {
                    clear_positions(child);
                }
            }
        }
        Item::None | Item::Value(_) => {}
    }
}

fn rewrite_item(item: &mut Item, rewrites: &[(String, String)]) {
    match item {
        Item::Value(value) => rewrite_value(value, rewrites),
        Item::Table(table) => {
            for (_, child) in table.iter_mut() {
                rewrite_item(child, rewrites);
            }
        }
        Item::ArrayOfTables(array) => {
            for table in array.iter_mut() {
                for (_, child) in table.iter_mut() {
                    rewrite_item(child, rewrites);
                }
            }
        }
        Item::None => {}
    }
}

fn rewrite_value(value: &mut Value, rewrites: &[(String, String)]) {
    match value {
        Value::String(text) => {
            if let Some(rewritten) = rewrite_str(text.value(), rewrites) {
                let decor = text.decor().clone();
                *value = Value::from(rewritten);
                *value.decor_mut() = decor;
            }
        }
        Value::Array(array) => {
            for element in array.iter_mut() {
                rewrite_value(element, rewrites);
            }
        }
        Value::InlineTable(table) => {
            for (_, element) in table.iter_mut() {
                rewrite_value(element, rewrites);
            }
        }
        Value::Integer(_) | Value::Float(_) | Value::Boolean(_) | Value::Datetime(_) => {}
    }
}

fn rewrite_str(text: &str, rewrites: &[(String, String)]) -> Option<String> {
    rewrites.iter().find_map(|(from, to)| {
        let rest = text.strip_prefix(from.as_str())?;
        (rest.is_empty() || rest.starts_with('/')).then(|| format!("{to}{rest}"))
    })
}

/// Hash of a setting's value that ignores formatting, comments and key order, and treats inline
/// and standard tables alike.
fn item_hash(item: &Item) -> String {
    content_hash(canonical_item(item).as_bytes())
}

fn canonical_item(item: &Item) -> String {
    match item {
        Item::None => String::new(),
        Item::Value(value) => canonical_value(value),
        Item::Table(table) => canonical_pairs(
            table
                .iter()
                .map(|(key, child)| (key.to_owned(), canonical_item(child))),
        ),
        Item::ArrayOfTables(array) => {
            let tables = array
                .iter()
                .map(|table| {
                    canonical_pairs(
                        table
                            .iter()
                            .map(|(key, child)| (key.to_owned(), canonical_item(child))),
                    )
                })
                .collect::<Vec<_>>();
            format!("[{}]", tables.join(","))
        }
    }
}

fn canonical_value(value: &Value) -> String {
    match value {
        Value::String(text) => format!("{:?}", text.value()),
        Value::Integer(int) => int.value().to_string(),
        Value::Float(float) => format!("{:?}", float.value()),
        Value::Boolean(boolean) => boolean.value().to_string(),
        Value::Datetime(datetime) => datetime.value().to_string(),
        Value::Array(array) => {
            let elements = array.iter().map(canonical_value).collect::<Vec<_>>();
            format!("[{}]", elements.join(","))
        }
        Value::InlineTable(table) => canonical_pairs(
            table
                .iter()
                .map(|(key, element)| (key.to_owned(), canonical_value(element))),
        ),
    }
}

fn canonical_pairs(pairs: impl Iterator<Item = (String, String)>) -> String {
    let mut pairs: Vec<_> = pairs.collect();
    pairs.sort();
    let body = pairs
        .into_iter()
        .map(|(key, value)| format!("{key:?}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{body}}}")
}

#[cfg(test)]
#[path = "settings_toml_tests.rs"]
mod tests;
