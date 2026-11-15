//! Trust for commands from a committed `.warpium/toolbar.toml`, and the other per-project state
//! kept in user preferences (never in the project).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{ButtonAction, CommandAction, CommandTarget, ToolbarButton};

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
    /// The [`command_signatures`] of the trusted team config, per project root, so the prompt for
    /// a changed file can tell what differs from the trusted version.
    #[serde(default)]
    trusted_commands: BTreeMap<String, BTreeMap<String, String>>,
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

    /// Trusts this version of the team config, whose merged buttons are `buttons`.
    pub fn trust(&mut self, root: &Path, team_hash: &str, buttons: &[ToolbarButton]) {
        self.trusted.insert(key(root), team_hash.to_owned());
        self.trusted_commands
            .insert(key(root), command_signatures(buttons));
    }

    /// The [`command_signatures`] of the last trusted version, if they were recorded.
    pub fn trusted_commands(&self, root: &Path) -> Option<&BTreeMap<String, String>> {
        self.trusted_commands.get(&key(root))
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

/// How a team command compares with the previously trusted version of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrustChange {
    /// No trusted command had this button id, or nothing was trusted before.
    New,
    /// The command, label, target, `cwd` or `env` differ from the trusted ones.
    Changed,
    Unchanged,
}

/// A team command as shown in the trust confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustListing {
    pub label: String,
    pub command: String,
    /// Where it runs and with which options, such as `new tab · cwd app · env GRADLE_OPTS`.
    pub details: String,
    pub change: TrustChange,
}

/// What the trust confirmation shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustReview {
    /// New and changed commands first, then unchanged ones, each group in file order.
    pub listings: Vec<TrustListing>,
    /// Whether an earlier version was trusted, so `listings` compare against it.
    pub is_update: bool,
    /// How many commands of the earlier trusted version are gone.
    pub removed: usize,
}

impl TrustReview {
    /// How many listings have this change.
    pub fn count(&self, change: TrustChange) -> usize {
        self.listings
            .iter()
            .filter(|listing| listing.change == change)
            .count()
    }
}

/// A fingerprint of each team command that needs trust, by button id, covering everything that
/// decides what runs (including `env` values) and its label.
pub fn command_signatures(buttons: &[ToolbarButton]) -> BTreeMap<String, String> {
    commands_needing_trust(buttons)
        .map(|(button, command)| (button.id.clone(), signature(button, command)))
        .collect()
}

/// Every command that trusting the team config would allow to run, compared with `previous`, the
/// [`ToolbarUserState::trusted_commands`] of the version trusted before (if any).
pub fn trust_review(
    buttons: &[ToolbarButton],
    previous: Option<&BTreeMap<String, String>>,
) -> TrustReview {
    let mut listings = commands_needing_trust(buttons)
        .map(|(button, command)| {
            let change = match previous.and_then(|previous| previous.get(&button.id)) {
                None => TrustChange::New,
                Some(trusted) if *trusted == signature(button, command) => TrustChange::Unchanged,
                Some(_) => TrustChange::Changed,
            };
            TrustListing {
                label: button.display_label().to_owned(),
                command: command.command.clone(),
                details: details(command),
                change,
            }
        })
        .collect::<Vec<_>>();
    // A stable sort keeps file order within each group.
    listings.sort_by_key(|listing| listing.change);
    let current = command_signatures(buttons);
    let removed = previous.map_or(0, |previous| {
        previous
            .keys()
            .filter(|id| !current.contains_key(*id))
            .count()
    });
    TrustReview {
        listings,
        is_update: previous.is_some(),
        removed,
    }
}

fn commands_needing_trust(
    buttons: &[ToolbarButton],
) -> impl Iterator<Item = (&ToolbarButton, &CommandAction)> {
    buttons
        .iter()
        .filter(|button| button.requires_trust())
        .filter_map(|button| match &button.action {
            ButtonAction::Command(command) => Some((button, command)),
            ButtonAction::Workflow(_)
            | ButtonAction::LaunchConfig(_)
            | ButtonAction::TabConfig(_)
            | ButtonAction::Url(_)
            | ButtonAction::Builtin(_) => None,
        })
}

fn signature(button: &ToolbarButton, command: &CommandAction) -> String {
    // JSON keeps the fields apart whatever characters they contain.
    let fields = serde_json::json!({
        "label": button.display_label(),
        "command": command.command,
        // The config's own names, so rewording the prompt doesn't change stored signatures.
        "target": match command.target {
            CommandTarget::Current => "current",
            CommandTarget::NewTab => "new_tab",
            CommandTarget::SplitRight => "split_right",
            CommandTarget::SplitDown => "split_down",
        },
        "cwd": command.cwd,
        "env": command.env,
    });
    content_hash(fields.to_string().as_bytes())
}

fn target_label(target: CommandTarget) -> &'static str {
    match target {
        CommandTarget::Current => "current session",
        CommandTarget::NewTab => "new tab",
        CommandTarget::SplitRight => "split right",
        CommandTarget::SplitDown => "split down",
    }
}

fn details(command: &CommandAction) -> String {
    let mut details = vec![target_label(command.target).to_owned()];
    if let Some(cwd) = &command.cwd {
        details.push(format!("cwd {cwd}"));
    }
    if !command.env.is_empty() {
        let names = command.env.keys().cloned().collect::<Vec<_>>().join(", ");
        details.push(format!("env {names}"));
    }
    details.join(" · ")
}

#[cfg(test)]
#[path = "trust_tests.rs"]
mod tests;
