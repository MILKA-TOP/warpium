use std::collections::BTreeMap;

use crate::report::ImportReport;
use crate::sync::{SyncDecision, content_hash, decide};

/// Private (user defaults) settings that are never imported, by storage key: account- or
/// machine-specific state, debug switches, and login-item bookkeeping that would make Warpium
/// fight Warp over the same OS registration.
pub const PRIVATE_SETTINGS_DENYLIST: &[&str] = &[
    "AIRequestQuotaInfoSetting",
    "AppAddedAsLoginItem",
    "AreInBandGeneratorsForAllSessionsEnabled",
    "DidNonAnonymousUserLogIn",
    "DisableInBandCommands",
    "IapRoutingEnabled",
    "IsShellDebugModeEnabled",
    "LastSelectedAuthSecret",
    "LastSelectedEnvironmentId",
    "RecordingModeEnabled",
    "ShowMemoryStats",
    "UserAppInstallationDetected",
];

/// The sync decision for one key of a string key-value store (the private settings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValuePlan {
    pub key: String,
    pub decision: SyncDecision,
    /// Warp's value, serialized as the settings framework stores it.
    pub source_value: Option<String>,
    source_hash: Option<String>,
}

impl KeyValuePlan {
    /// Records the outcome of applying this plan in the manifest map and the report.
    ///
    /// `applied` is the result of writing Warp's value (for decisions that take the source) or of
    /// resetting the value (for [`SyncDecision::Remove`]); it is ignored otherwise.
    pub fn finish(
        &self,
        applied: Result<(), String>,
        base: &mut BTreeMap<String, String>,
        report: &mut ImportReport,
    ) {
        let key = self.key.clone();
        match self.decision {
            SyncDecision::Create | SyncDecision::Update | SyncDecision::Overwrite => match applied
            {
                Ok(()) => {
                    if let Some(hash) = &self.source_hash {
                        base.insert(key.clone(), hash.clone());
                    }
                    if self.decision == SyncDecision::Create {
                        report.imported.push(key);
                    } else {
                        report.updated.push(key);
                    }
                }
                Err(err) => report.errors.push((key, err)),
            },
            SyncDecision::Remove => match applied {
                Ok(()) => {
                    base.remove(&key);
                    report.removed.push(key);
                }
                Err(err) => report.errors.push((key, err)),
            },
            SyncDecision::InSync => {
                if let Some(hash) = &self.source_hash {
                    base.insert(key, hash.clone());
                }
                report.unchanged += 1;
            }
            SyncDecision::KeepLocal => {
                if self.source_hash.is_none() {
                    base.remove(&key);
                }
                report.unchanged += 1;
            }
            SyncDecision::Conflict => report.conflicts.push(key),
        }
    }
}

/// Plans the 3-way sync of `keys` between Warp's values (`source`) and Warpium's (`target`).
/// Denylisted keys are left out.
pub fn plan_key_values(
    keys: &[String],
    source: &BTreeMap<String, String>,
    target: &BTreeMap<String, String>,
    base: &BTreeMap<String, String>,
    initial_import: bool,
) -> Vec<KeyValuePlan> {
    keys.iter()
        .filter(|key| !PRIVATE_SETTINGS_DENYLIST.contains(&key.as_str()))
        .map(|key| {
            let source_value = source.get(key).cloned();
            let source_hash = source_value.as_deref().map(value_hash);
            let target_hash = target.get(key).map(|value| value_hash(value));
            let decision = decide(
                base.get(key).map(String::as_str),
                source_hash.as_deref(),
                target_hash.as_deref(),
                initial_import,
            );
            KeyValuePlan {
                key: key.clone(),
                decision,
                source_value,
                source_hash,
            }
        })
        .collect()
}

/// Hash of a stored value that ignores JSON object key order and whitespace.
fn value_hash(value: &str) -> String {
    let canonical = serde_json::from_str::<serde_json::Value>(value)
        .map(|json| canonical_json(&json))
        .unwrap_or_else(|_| value.to_owned());
    content_hash(canonical.as_bytes())
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let body = entries
                .into_iter()
                .map(|(key, value)| format!("{key:?}:{}", canonical_json(value)))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        serde_json::Value::Array(items) => {
            let body = items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{body}]")
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => value.to_string(),
    }
}

#[cfg(test)]
#[path = "key_values_tests.rs"]
mod tests;
