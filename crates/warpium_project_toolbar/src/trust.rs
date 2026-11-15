//! Trust for commands from a committed `.warpium/toolbar.toml`, and the other per-project state
//! kept in user preferences (never in the project).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{ButtonAction, CommandTarget, ToolbarButton};

/// Identifies one version of a team config file.
pub fn content_hash(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Toolbar state remembered across launches, keyed by project root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolbarUserState {
    /// The [`content_hash`] of the team config the user trusted, per project root.
    #[serde(default)]
    trusted: BTreeMap<String, String>,
    /// The selected run configuration key, per project root.
    #[serde(default)]
    selected_run_config: BTreeMap<String, String>,
}

impl ToolbarUserState {
    /// Reads saved state; unreadable state starts over (nothing trusted).
    pub fn from_json(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Whether the user trusted exactly this version of the project's team config.
    pub fn is_trusted(&self, root: &Path, team_hash: &str) -> bool {
        self.trusted
            .get(&key(root))
            .is_some_and(|trusted| trusted == team_hash)
    }

    pub fn trust(&mut self, root: &Path, team_hash: &str) {
        self.trusted.insert(key(root), team_hash.to_owned());
    }

    pub fn selected_run_config(&self, root: &Path) -> Option<&str> {
        self.selected_run_config.get(&key(root)).map(String::as_str)
    }

    pub fn select_run_config(&mut self, root: &Path, run_config_key: &str) {
        self.selected_run_config
            .insert(key(root), run_config_key.to_owned());
    }
}

fn key(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

/// Whether running `button` must first ask the user to trust the team config.
pub fn needs_trust(
    button: &ToolbarButton,
    root: &Path,
    team_hash: Option<&str>,
    state: &ToolbarUserState,
) -> bool {
    if !button.requires_trust() {
        return false;
    }
    match team_hash {
        Some(hash) => !state.is_trusted(root, hash),
        // A team command whose file can no longer be hashed is never trusted.
        None => true,
    }
}

/// A team command as shown in the trust confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustListing {
    pub label: String,
    pub command: String,
    /// Where it runs and with which options, such as `new tab · cwd app · env GRADLE_OPTS`.
    pub details: String,
}

/// Every command that trusting the team config would allow to run.
pub fn trust_listings(buttons: &[ToolbarButton]) -> Vec<TrustListing> {
    buttons
        .iter()
        .filter(|button| button.requires_trust())
        .filter_map(|button| {
            let ButtonAction::Command(command) = &button.action else {
                return None;
            };
            let mut details = vec![
                match command.target {
                    CommandTarget::Current => "current session",
                    CommandTarget::NewTab => "new tab",
                    CommandTarget::SplitRight => "split right",
                    CommandTarget::SplitDown => "split down",
                }
                .to_owned(),
            ];
            if let Some(cwd) = &command.cwd {
                details.push(format!("cwd {cwd}"));
            }
            if !command.env.is_empty() {
                let names = command.env.keys().cloned().collect::<Vec<_>>().join(", ");
                details.push(format!("env {names}"));
            }
            Some(TrustListing {
                label: button.display_label().to_owned(),
                command: command.command.clone(),
                details: details.join(" · "),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "trust_tests.rs"]
mod tests;
