//! Trust for the commands of shared run configurations, which come from the repository.

use std::collections::{BTreeMap, BTreeSet};

use warpium_project_toolbar::{
    CommandAction, TrustChange, TrustListing, TrustReview, content_hash,
};

use crate::model::{IdeRunConfig, RunStep, StepAction, type_label};

/// Whether running `config` runs commands from a shared file, which the user has to trust first.
/// Android runs are covered by the project's Gradle build trust instead.
pub fn needs_trust(config: &IdeRunConfig) -> bool {
    config
        .steps()
        .is_some_and(|steps| steps.iter().any(|step| shared_command(step).is_some()))
}

/// A fingerprint of each shared command, by the key of its configuration, covering everything
/// that decides what runs and its name.
pub fn trust_signatures(configs: &[IdeRunConfig]) -> BTreeMap<String, String> {
    shared_commands(configs)
        .map(|(step, command)| (step.config_key.clone(), signature(step, command)))
        .collect()
}

/// Identifies the set of shared commands the user trusts at once; `None` when there are none.
/// It changes whenever any of them does, so trust is asked for again.
pub fn trust_fingerprint(configs: &[IdeRunConfig]) -> Option<String> {
    let signatures = trust_signatures(configs);
    (!signatures.is_empty()).then(|| {
        content_hash(
            serde_json::to_string(&signatures)
                .unwrap_or_default()
                .as_bytes(),
        )
    })
}

/// Every shared command trusting would allow to run, compared with `previous`, the
/// [`trust_signatures`] of the set trusted before (if any). New and changed ones come first.
pub fn trust_review(
    configs: &[IdeRunConfig],
    previous: Option<&BTreeMap<String, String>>,
) -> TrustReview {
    let mut listings = shared_commands(configs)
        .map(|(step, command)| {
            let change = match previous.and_then(|previous| previous.get(&step.config_key)) {
                None => TrustChange::New,
                Some(trusted) if *trusted == signature(step, command) => TrustChange::Unchanged,
                Some(_) => TrustChange::Changed,
            };
            let config = configs.iter().find(|config| config.key == step.config_key);
            TrustListing {
                label: step.label.clone(),
                command: command.command.clone(),
                details: details(config, command),
                change,
            }
        })
        .collect::<Vec<_>>();
    // A stable sort keeps file order within each group.
    listings.sort_by_key(|listing| listing.change);
    let current = shared_commands(configs)
        .map(|(step, _)| step.config_key.as_str())
        .collect::<BTreeSet<_>>();
    let removed = previous.map_or(0, |previous| {
        previous
            .keys()
            .filter(|key| !current.contains(key.as_str()))
            .count()
    });
    TrustReview {
        listings,
        is_update: previous.is_some(),
        removed,
    }
}

/// The shared command steps of all configurations, each configuration's once (a compound's
/// members are configurations of their own too).
fn shared_commands(configs: &[IdeRunConfig]) -> impl Iterator<Item = (&RunStep, &CommandAction)> {
    let mut seen = BTreeSet::new();
    configs
        .iter()
        .filter_map(IdeRunConfig::steps)
        .flatten()
        .filter_map(shared_command)
        .filter(move |(step, _)| seen.insert(step.config_key.clone()))
}

fn shared_command(step: &RunStep) -> Option<(&RunStep, &CommandAction)> {
    match &step.action {
        StepAction::Command(command) if step.shared => Some((step, command)),
        StepAction::Command(_) | StepAction::AndroidApp(_) => None,
    }
}

fn signature(step: &RunStep, command: &CommandAction) -> String {
    // JSON keeps the fields apart whatever characters they contain.
    let fields = serde_json::json!({
        "label": step.label,
        "command": command.command,
        "cwd": command.cwd,
        "env": command.env,
    });
    content_hash(fields.to_string().as_bytes())
}

/// Such as `Gradle · .run/detekt.run.xml · cwd app · env JAVA_HOME`.
fn details(config: Option<&IdeRunConfig>, command: &CommandAction) -> String {
    let mut details = Vec::new();
    if let Some(config) = config {
        details.push(type_label(&config.type_id).to_owned());
        if let crate::model::ConfigOrigin::Shared(file) = &config.origin {
            details.push(file.clone());
        }
    }
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
