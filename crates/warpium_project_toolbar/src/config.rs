//! The `toolbar.toml` format: parsing, validation and the merge of personal overrides into the
//! team config.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::Path;

use serde::Deserialize;

use crate::action::{BuiltinAction, is_valid_env_name};
use crate::icons::{ButtonIcon, is_contained_relative_path, parse_icon};

/// Which file a config, or a button's action, came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// The committed `.warpium/toolbar.toml`.
    Team,
    /// The user's own overrides in `~/.warp-oss/projects/<project-id>/toolbar.toml`.
    Personal,
}

impl ConfigSource {
    fn file_description(self) -> &'static str {
        match self {
            ConfigSource::Team => ".warpium/toolbar.toml",
            ConfigSource::Personal => "personal toolbar overrides",
        }
    }
}

/// A config file that can't be used, with a message for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub source: ConfigSource,
    pub message: String,
}

impl ConfigError {
    fn new(source: ConfigSource, message: impl Into<String>) -> Self {
        Self {
            source,
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid {}: {}",
            self.source.file_description(),
            self.message
        )
    }
}

impl std::error::Error for ConfigError {}

/// Where a `command` button runs its command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandTarget {
    /// The active terminal session.
    #[default]
    Current,
    NewTab,
    SplitRight,
    SplitDown,
}

impl CommandTarget {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "current" => Ok(Self::Current),
            "new_tab" => Ok(Self::NewTab),
            "split_right" => Ok(Self::SplitRight),
            "split_down" => Ok(Self::SplitDown),
            other => Err(format!(
                "unknown target `{other}`: expected `current`, `new_tab`, `split_right` or \
                 `split_down`"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::NewTab => "new_tab",
            Self::SplitRight => "split_right",
            Self::SplitDown => "split_down",
        }
    }
}

/// A shell command, run as an ordinary block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandAction {
    pub command: String,
    pub target: CommandTarget,
    /// Relative to the project root; the root when absent.
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
}

/// What clicking a button does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ButtonAction {
    Command(CommandAction),
    /// Runs a saved workflow, by name.
    Workflow(String),
    /// Opens a launch configuration, by name.
    LaunchConfig(String),
    /// Opens a tab config, by name.
    TabConfig(String),
    /// Opens an `http(s)` URL.
    Url(String),
    Builtin(BuiltinAction),
}

/// A button after the personal overrides were merged in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolbarButton {
    pub id: String,
    pub label: Option<String>,
    pub icon: Option<ButtonIcon>,
    pub tooltip: Option<String>,
    /// Listed in the run-configurations dropdown instead of the toolbar.
    pub run_config: bool,
    pub action: ButtonAction,
    /// The file the action came from. Commands from the team file need the user's trust.
    pub action_source: ConfigSource,
}

impl ToolbarButton {
    /// The label, or the id when there is none.
    pub fn display_label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }

    /// The tooltip text: the tooltip, else the label, else a description of the action.
    pub fn tooltip_text(&self) -> String {
        if let Some(tooltip) = self.tooltip.as_ref().or(self.label.as_ref()) {
            return tooltip.clone();
        }
        match &self.action {
            ButtonAction::Command(command) => command.command.clone(),
            ButtonAction::Workflow(name) => format!("Run workflow \"{name}\""),
            ButtonAction::LaunchConfig(name) => format!("Open launch configuration \"{name}\""),
            ButtonAction::TabConfig(name) => format!("Open tab config \"{name}\""),
            ButtonAction::Url(url) => format!("Open {url}"),
            ButtonAction::Builtin(action) => action.id().to_owned(),
        }
    }

    /// Whether running this button needs the project's toolbar to be trusted first.
    pub fn requires_trust(&self) -> bool {
        matches!(self.action, ButtonAction::Command(_))
            && self.action_source == ConfigSource::Team
    }
}

/// One parsed `toolbar.toml`, before merging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedConfig {
    source: ConfigSource,
    entries: Vec<PartialButton>,
}

impl ParsedConfig {
    pub fn source(&self) -> ConfigSource {
        self.source
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    button: Vec<RawButton>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawButton {
    id: Option<String>,
    label: Option<String>,
    icon: Option<String>,
    tooltip: Option<String>,
    hidden: Option<bool>,
    run_config: Option<bool>,
    command: Option<String>,
    target: Option<String>,
    cwd: Option<String>,
    env: Option<BTreeMap<String, String>>,
    workflow: Option<String>,
    launch_config: Option<String>,
    tab_config: Option<String>,
    url: Option<String>,
    action: Option<String>,
}

/// The action key of one entry, before `target`, `cwd` and `env` are attached.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ActionSpec {
    Command(String),
    Workflow(String),
    LaunchConfig(String),
    TabConfig(String),
    Url(String),
    Builtin(BuiltinAction),
}

/// One validated `[[button]]` entry. In the personal file an entry that overrides a team button
/// may set only some fields.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PartialButton {
    id: String,
    label: Option<String>,
    icon: Option<ButtonIcon>,
    tooltip: Option<String>,
    hidden: bool,
    run_config: Option<bool>,
    action: Option<ActionSpec>,
    target: Option<CommandTarget>,
    cwd: Option<String>,
    env: Option<BTreeMap<String, String>>,
    action_source: ConfigSource,
}

/// Parses one `toolbar.toml`. Team entries must be complete buttons; personal entries may be
/// partial overrides of team buttons.
pub fn parse_config(text: &str, source: ConfigSource) -> Result<ParsedConfig, ConfigError> {
    let raw: RawFile = toml::from_str(text)
        .map_err(|err| ConfigError::new(source, err.to_string().trim_end().to_owned()))?;
    let mut seen = HashSet::new();
    let mut entries = Vec::with_capacity(raw.button.len());
    for (index, raw_button) in raw.button.into_iter().enumerate() {
        let entry = parse_entry(raw_button, source)
            .map_err(|message| ConfigError::new(source, format!("button {}: {message}", index + 1)))?;
        if !seen.insert(entry.id.clone()) {
            return Err(ConfigError::new(
                source,
                format!("button id `{}` is used more than once", entry.id),
            ));
        }
        if source == ConfigSource::Team && !entry.hidden && entry.action.is_none() {
            return Err(ConfigError::new(
                source,
                format!("button `{}` has no action", entry.id),
            ));
        }
        entries.push(entry);
    }
    Ok(ParsedConfig { source, entries })
}

fn parse_entry(raw: RawButton, source: ConfigSource) -> Result<PartialButton, String> {
    let id = raw
        .id
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "every button needs a non-empty `id`".to_owned())?;
    let in_button = |message: String| format!("`{id}`: {message}");

    let mut actions = Vec::new();
    if let Some(command) = raw.command {
        if command.trim().is_empty() {
            return Err(in_button("`command` is empty".to_owned()));
        }
        actions.push(ActionSpec::Command(command));
    }
    for (key, value, make) in [
        ("workflow", raw.workflow, ActionSpec::Workflow as fn(String) -> ActionSpec),
        ("launch_config", raw.launch_config, ActionSpec::LaunchConfig),
        ("tab_config", raw.tab_config, ActionSpec::TabConfig),
    ] {
        if let Some(name) = value {
            if name.trim().is_empty() {
                return Err(in_button(format!("`{key}` is empty")));
            }
            actions.push(make(name));
        }
    }
    if let Some(url) = raw.url {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(in_button(format!(
                "`url` must start with https:// or http://, got `{url}`"
            )));
        }
        actions.push(ActionSpec::Url(url));
    }
    if let Some(action) = raw.action {
        let builtin = BuiltinAction::from_id(&action).ok_or_else(|| {
            in_button(format!(
                "unknown action `{action}`; supported actions: {}",
                BuiltinAction::ALL
                    .iter()
                    .map(|action| action.id())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        actions.push(ActionSpec::Builtin(builtin));
    }
    if actions.len() > 1 {
        return Err(in_button(
            "set only one of `command`, `workflow`, `launch_config`, `tab_config`, `url` and \
             `action`"
                .to_owned(),
        ));
    }
    let action = actions.pop();

    let target = raw
        .target
        .as_deref()
        .map(CommandTarget::parse)
        .transpose()
        .map_err(in_button)?;
    if let Some(cwd) = &raw.cwd
        && !is_contained_relative_path(Path::new(cwd))
    {
        return Err(in_button(format!(
            "`cwd` must be relative to the project root and stay inside it, got `{cwd}`"
        )));
    }
    if let Some(env) = &raw.env
        && let Some(name) = env.keys().find(|name| !is_valid_env_name(name))
    {
        return Err(in_button(format!("`{name}` is not a valid environment variable name")));
    }
    let has_command_options = target.is_some() || raw.cwd.is_some() || raw.env.is_some();
    if has_command_options && action.as_ref().is_some_and(|a| !matches!(a, ActionSpec::Command(_)))
    {
        return Err(in_button(
            "`target`, `cwd` and `env` only apply to `command` buttons".to_owned(),
        ));
    }
    let icon = raw.icon.as_deref().map(parse_icon).transpose().map_err(in_button)?;

    Ok(PartialButton {
        id,
        label: raw.label,
        icon,
        tooltip: raw.tooltip,
        hidden: raw.hidden.unwrap_or(false),
        run_config: raw.run_config,
        action,
        target,
        cwd: raw.cwd,
        env: raw.env,
        action_source: source,
    })
}

/// Merges the personal overrides into the team config by button `id`: a personal entry with a
/// team button's id overrides the fields it sets (an action key replaces the whole action) or,
/// with `hidden = true`, removes the button; other personal entries add buttons after the team's.
pub fn merge(
    team: Option<&ParsedConfig>,
    personal: Option<&ParsedConfig>,
) -> Result<Vec<ToolbarButton>, ConfigError> {
    let mut merged: Vec<PartialButton> = team.map(|team| team.entries.clone()).unwrap_or_default();
    if let Some(personal) = personal {
        for entry in &personal.entries {
            match merged.iter_mut().find(|button| button.id == entry.id) {
                Some(base) => apply_override(base, entry),
                None => merged.push(entry.clone()),
            }
        }
    }
    merged
        .into_iter()
        .filter(|button| !button.hidden)
        .map(finish)
        .collect()
}

fn apply_override(base: &mut PartialButton, entry: &PartialButton) {
    if entry.hidden {
        base.hidden = true;
        return;
    }
    if entry.label.is_some() {
        base.label = entry.label.clone();
    }
    if entry.icon.is_some() {
        base.icon = entry.icon.clone();
    }
    if entry.tooltip.is_some() {
        base.tooltip = entry.tooltip.clone();
    }
    if entry.run_config.is_some() {
        base.run_config = entry.run_config;
    }
    if entry.action.is_some() {
        base.action = entry.action.clone();
        base.target = entry.target;
        base.cwd = entry.cwd.clone();
        base.env = entry.env.clone();
        base.action_source = entry.action_source;
    } else {
        // Options of a team command, changed personally: the command itself is still the
        // team's, so it keeps needing trust.
        if entry.target.is_some() {
            base.target = entry.target;
        }
        if entry.cwd.is_some() {
            base.cwd = entry.cwd.clone();
        }
        if entry.env.is_some() {
            base.env = entry.env.clone();
        }
    }
}

fn finish(button: PartialButton) -> Result<ToolbarButton, ConfigError> {
    let error = |message: String| ConfigError::new(button.action_source, message);
    let Some(action) = button.action else {
        return Err(error(format!("button `{}` has no action", button.id)));
    };
    let has_command_options =
        button.target.is_some() || button.cwd.is_some() || button.env.is_some();
    if has_command_options && !matches!(action, ActionSpec::Command(_)) {
        return Err(error(format!(
            "button `{}`: `target`, `cwd` and `env` only apply to `command` buttons",
            button.id
        )));
    }
    let action = match action {
        ActionSpec::Command(command) => ButtonAction::Command(CommandAction {
            command,
            target: button.target.unwrap_or_default(),
            cwd: button.cwd,
            env: button.env.unwrap_or_default(),
        }),
        ActionSpec::Workflow(name) => ButtonAction::Workflow(name),
        ActionSpec::LaunchConfig(name) => ButtonAction::LaunchConfig(name),
        ActionSpec::TabConfig(name) => ButtonAction::TabConfig(name),
        ActionSpec::Url(url) => ButtonAction::Url(url),
        ActionSpec::Builtin(builtin) => ButtonAction::Builtin(builtin),
    };
    Ok(ToolbarButton {
        id: button.id,
        label: button.label,
        icon: button.icon,
        tooltip: button.tooltip,
        run_config: button.run_config.unwrap_or(false),
        action,
        action_source: button.action_source,
    })
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
