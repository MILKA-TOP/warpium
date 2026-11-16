//! Editing the toolbar from the GUI: an editable model of both config files' buttons, and
//! writing it back with `toml_edit` so comments, key order and formatting the user wrote stay
//! where they were.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use crate::action::{BuiltinAction, is_valid_env_name};
use crate::config::{
    ActionSpec, ButtonAction, CommandAction, CommandTarget, ConfigError, ConfigSource,
    PartialButton, ToolbarButton, apply_override, merge, parse_config,
};
use crate::icons::{ButtonIcon, is_contained_relative_path, parse_icon};

/// The keys that each name one kind of action.
const ACTION_KEYS: &[&str] = &[
    "command",
    "workflow",
    "launch_config",
    "tab_config",
    "url",
    "action",
];

/// Where a button shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    /// Inline on the toolbar while it fits, then in the overflow menu.
    #[default]
    Toolbar,
    /// Always in the overflow menu (`overflow = true`).
    Overflow,
    /// In the run-configurations dropdown (`run_config = true`).
    RunConfig,
}

/// What an edited button does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecAction {
    Command {
        command: String,
        target: CommandTarget,
        /// Relative to the project root.
        cwd: Option<String>,
        /// In the order the editor shows them.
        env: Vec<(String, String)>,
    },
    Workflow(String),
    LaunchConfig(String),
    TabConfig(String),
    Url(String),
    /// A [`BuiltinAction`] id.
    Builtin(String),
}

/// A complete button as the editor shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonSpec {
    pub id: String,
    pub label: Option<String>,
    /// A bundled icon name or an `.svg` path relative to `.warpium/`, as written in the file.
    pub icon: Option<String>,
    pub tooltip: Option<String>,
    pub placement: Placement,
    pub action: SpecAction,
}

impl ButtonSpec {
    /// A new shell-command button with an empty command.
    pub fn new_command(id: String) -> Self {
        Self {
            id,
            label: None,
            icon: None,
            tooltip: None,
            placement: Placement::Toolbar,
            action: SpecAction::Command {
                command: String::new(),
                target: CommandTarget::Current,
                cwd: None,
                env: Vec::new(),
            },
        }
    }

    /// The label, or the id when there is none.
    pub fn display_label(&self) -> &str {
        self.label
            .as_deref()
            .filter(|label| !label.trim().is_empty())
            .unwrap_or(&self.id)
    }

    /// Checks the button as the config parser would, with a message for the editor.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("every button needs an id".to_owned());
        }
        if let Some(icon) = non_empty(&self.icon) {
            parse_icon(icon)?;
        }
        let required = |kind: &str, value: &str| {
            if value.trim().is_empty() {
                Err(format!("enter the {kind}"))
            } else {
                Ok(())
            }
        };
        match &self.action {
            SpecAction::Command {
                command, cwd, env, ..
            } => {
                required("command", command)?;
                if let Some(cwd) = non_empty(cwd)
                    && !is_contained_relative_path(Path::new(cwd))
                {
                    return Err(format!(
                        "the working directory must be relative to the project root and stay \
                         inside it, got `{cwd}`"
                    ));
                }
                let mut names = BTreeSet::new();
                for (name, _) in env {
                    if !is_valid_env_name(name) {
                        return Err(format!("`{name}` is not a valid environment variable name"));
                    }
                    if !names.insert(name) {
                        return Err(format!("the environment variable `{name}` is set twice"));
                    }
                }
                Ok(())
            }
            SpecAction::Workflow(name) => required("workflow name", name),
            SpecAction::LaunchConfig(name) => required("launch configuration name", name),
            SpecAction::TabConfig(name) => required("tab config name", name),
            SpecAction::Url(url) => {
                if url.starts_with("https://") || url.starts_with("http://") {
                    Ok(())
                } else {
                    Err("the URL must start with https:// or http://".to_owned())
                }
            }
            SpecAction::Builtin(id) => match BuiltinAction::from_id(id) {
                Some(_) => Ok(()),
                None => Err("choose a built-in action".to_owned()),
            },
        }
    }

    /// The button as the toolbar would run it, as if it came from `source`.
    pub fn to_toolbar_button(&self, source: ConfigSource) -> Result<ToolbarButton, String> {
        self.validate()?;
        let action = match &self.action {
            SpecAction::Command {
                command,
                target,
                cwd,
                env,
            } => ButtonAction::Command(CommandAction {
                command: command.clone(),
                target: *target,
                cwd: non_empty(cwd).map(str::to_owned),
                env: env.iter().cloned().collect(),
            }),
            SpecAction::Workflow(name) => ButtonAction::Workflow(name.clone()),
            SpecAction::LaunchConfig(name) => ButtonAction::LaunchConfig(name.clone()),
            SpecAction::TabConfig(name) => ButtonAction::TabConfig(name.clone()),
            SpecAction::Url(url) => ButtonAction::Url(url.clone()),
            SpecAction::Builtin(id) => ButtonAction::Builtin(
                BuiltinAction::from_id(id).ok_or_else(|| "choose a built-in action".to_owned())?,
            ),
        };
        Ok(ToolbarButton {
            id: self.id.trim().to_owned(),
            label: non_empty(&self.label).map(str::to_owned),
            icon: non_empty(&self.icon).map(parse_icon).transpose()?,
            tooltip: non_empty(&self.tooltip).map(str::to_owned),
            run_config: self.placement == Placement::RunConfig,
            overflow_only: self.placement == Placement::Overflow,
            action,
            action_source: source,
        })
    }
}

/// Which file a button is saved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The shared `.warpium/toolbar.toml`.
    Project,
    /// The user's own overrides file.
    Personal,
}

/// A button in the editor's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorButton {
    pub spec: ButtonSpec,
    pub scope: Scope,
    /// Hidden from the toolbar for this user (`hidden = true` in the personal file).
    pub hidden: bool,
}

/// The new contents of the config files; `None` leaves a file as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavePlan {
    pub team: Option<String>,
    pub personal: Option<String>,
}

/// Why the editor's buttons can't be saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveError {
    /// The index of the offending button in [`EditorModel::buttons`], when it is one button.
    pub button: Option<usize>,
    pub message: String,
}

impl SaveError {
    fn file(message: String) -> Self {
        Self {
            button: None,
            message,
        }
    }
}

/// The buttons of both config files, as the editor edits them.
///
/// The list is in toolbar order: the buttons whose entry lives in the team file (project
/// buttons and the personal overrides of project buttons), then the personal-only buttons. The
/// merge always shows them in that order, so moves never cross between the two groups.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditorModel {
    pub buttons: Vec<EditorButton>,
    /// The ids of the team file's buttons when the model was loaded.
    team_ids: BTreeSet<String>,
    /// The team file's own version of each of its buttons, without personal overrides.
    team_specs: BTreeMap<String, ButtonSpec>,
}

impl EditorModel {
    /// Reads both files' buttons. A personal entry that changes a project button makes that
    /// button personal (its changes are saved as a personal override).
    pub fn load(team_text: Option<&str>, personal_text: Option<&str>) -> Result<Self, ConfigError> {
        let team = team_text
            .map(|text| parse_config(text, ConfigSource::Team))
            .transpose()?;
        let personal = personal_text
            .map(|text| parse_config(text, ConfigSource::Personal))
            .transpose()?;
        let personal_entries = personal
            .as_ref()
            .map(|personal| personal.entries.as_slice())
            .unwrap_or_default();
        let mut model = Self::default();
        for entry in team.iter().flat_map(|team| &team.entries) {
            model.team_ids.insert(entry.id.clone());
            // Buttons the team file itself hides aren't shown; saving keeps their entries.
            if entry.hidden {
                continue;
            }
            if let Some(spec) = spec_from_entry(entry) {
                model.team_specs.insert(entry.id.clone(), spec);
            }
            let mut merged = entry.clone();
            let mut scope = Scope::Project;
            let mut hidden = false;
            if let Some(over) = personal_entries.iter().find(|over| over.id == entry.id) {
                hidden |= over.hidden;
                if over.overrides_fields() {
                    let mut fields = over.clone();
                    fields.hidden = false;
                    apply_override(&mut merged, &fields);
                    scope = Scope::Personal;
                }
            }
            if let Some(spec) = spec_from_entry(&merged) {
                model.buttons.push(EditorButton {
                    spec,
                    scope,
                    hidden,
                });
            }
        }
        for entry in personal_entries {
            if model.team_ids.contains(&entry.id) {
                continue;
            }
            if let Some(spec) = spec_from_entry(entry) {
                model.buttons.push(EditorButton {
                    spec,
                    scope: Scope::Personal,
                    hidden: entry.hidden,
                });
            }
        }
        Ok(model)
    }

    /// Whether the button's entry lives in the team file, which decides its group.
    pub fn is_team_located(&self, index: usize) -> bool {
        self.buttons
            .get(index)
            .is_some_and(|button| self.located_in_team(button))
    }

    fn located_in_team(&self, button: &EditorButton) -> bool {
        button.scope == Scope::Project || self.team_ids.contains(button.spec.id.trim())
    }

    /// Whether the button was in the team file when the model was loaded.
    pub fn is_in_team_file(&self, id: &str) -> bool {
        self.team_ids.contains(id)
    }

    /// Whether the button runs the team file's own action (so running it needs trust in the
    /// team file), rather than one the user wrote.
    pub fn runs_team_action(&self, index: usize) -> bool {
        self.buttons.get(index).is_some_and(|button| {
            self.team_specs
                .get(button.spec.id.trim())
                .is_some_and(|team| team.action == button.spec.action)
        })
    }

    /// Whether the button is a personal override of a project button.
    pub fn is_override(&self, index: usize) -> bool {
        self.buttons.get(index).is_some_and(|button| {
            button.scope == Scope::Personal && self.team_specs.contains_key(button.spec.id.trim())
        })
    }

    /// Drops the personal changes to a project button, showing the team's version again.
    pub fn revert_override(&mut self, index: usize) {
        let Some(button) = self.buttons.get_mut(index) else {
            return;
        };
        if let Some(team) = self.team_specs.get(button.spec.id.trim()) {
            button.spec = team.clone();
            button.scope = Scope::Project;
        }
    }

    /// Puts the buttons back in toolbar order (team-file buttons first), keeping their relative
    /// order, after a scope change.
    pub fn normalize(&mut self) {
        let buttons = std::mem::take(&mut self.buttons);
        let (team, personal): (Vec<_>, Vec<_>) = buttons
            .into_iter()
            .partition(|button| self.located_in_team(button));
        self.buttons = team;
        self.buttons.extend(personal);
    }

    /// The index the button at `index` moves to by `delta`, if it can move there.
    pub fn move_target(&self, index: usize, delta: isize) -> Option<usize> {
        let target = index.checked_add_signed(delta)?;
        (index < self.buttons.len()
            && target < self.buttons.len()
            && self.is_team_located(index) == self.is_team_located(target))
        .then_some(target)
    }

    /// Moves a button by `delta` places within its group; returns its new index.
    pub fn move_button(&mut self, index: usize, delta: isize) -> Option<usize> {
        let target = self.move_target(index, delta)?;
        let button = self.buttons.remove(index);
        self.buttons.insert(target, button);
        Some(target)
    }

    /// Adds a button at the end of its group; returns its index.
    pub fn add(&mut self, spec: ButtonSpec, scope: Scope) -> usize {
        let id = spec.id.clone();
        self.buttons.push(EditorButton {
            spec,
            scope,
            hidden: false,
        });
        self.normalize();
        self.index_of(&id).unwrap_or(self.buttons.len() - 1)
    }

    /// Adds a copy of a button with a new id after it; returns the copy's index. A copy of a
    /// project button (or of a personal override of one) goes in the team file, so a copied team
    /// command still needs trust.
    pub fn duplicate(&mut self, index: usize) -> Option<usize> {
        let mut original = self.buttons.get(index)?.clone();
        if self.located_in_team(&original) {
            original.scope = Scope::Project;
        }
        let id = self.unique_id(&format!("{}-copy", original.spec.id));
        let mut spec = original.spec.clone();
        spec.id = id.clone();
        spec.label = original.spec.label.map(|label| format!("{label} copy"));
        self.buttons.insert(
            index + 1,
            EditorButton {
                spec,
                scope: original.scope,
                hidden: false,
            },
        );
        self.normalize();
        self.index_of(&id)
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.buttons.len() {
            self.buttons.remove(index);
        }
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.buttons
            .iter()
            .position(|button| button.spec.id.trim() == id)
    }

    /// An id based on `base` (made of letters, digits and dashes) that no button uses yet.
    pub fn unique_id(&self, base: &str) -> String {
        let base = slug(base);
        let taken = |id: &str| {
            self.buttons
                .iter()
                .any(|button| button.spec.id.trim() == id)
                || self.team_ids.contains(id)
        };
        if !taken(&base) {
            return base;
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|id| !taken(id))
            .expect("some numbered id is free")
    }

    /// The new contents of the config files for the edited buttons, given the files as they are
    /// now. Each button is written in place over its existing entry, so comments and formatting
    /// around it stay; a file is only created when it gets a button.
    pub fn plan_save(
        &self,
        team_text: Option<&str>,
        personal_text: Option<&str>,
    ) -> Result<SavePlan, SaveError> {
        let mut ids = BTreeSet::new();
        for (index, button) in self.buttons.iter().enumerate() {
            let error = |message: String| SaveError {
                button: Some(index),
                message: format!("{}: {message}", button.spec.display_label()),
            };
            button.spec.validate().map_err(&error)?;
            if !ids.insert(button.spec.id.trim()) {
                return Err(error(format!(
                    "the id `{}` is used by another button",
                    button.spec.id.trim()
                )));
            }
        }

        let mut team_doc = parse_document(team_text, ConfigSource::Team)?;
        let had_team_buttons = team_doc.contains_key("button");
        let mut team_entries = take_button_tables(&mut team_doc);
        let mut team_tables = Vec::new();
        for button in self.buttons.iter().filter(|b| self.located_in_team(b)) {
            let existing = take_entry(&mut team_entries, button.spec.id.trim());
            match button.scope {
                Scope::Project => {
                    let mut table = existing.unwrap_or_else(new_entry);
                    write_spec(&mut table, &button.spec);
                    team_tables.push(table);
                }
                // A personal override leaves the project's own entry as it is.
                Scope::Personal => team_tables.extend(existing),
            }
        }
        // Entries the team file hides itself aren't in the editor; keep them.
        team_tables.extend(
            team_entries
                .into_iter()
                .filter(|table| table.get("hidden").and_then(Item::as_bool) == Some(true)),
        );
        put_button_tables(&mut team_doc, team_tables, had_team_buttons);

        let mut personal_doc = parse_document(personal_text, ConfigSource::Personal)?;
        let had_personal_buttons = personal_doc.contains_key("button");
        let mut personal_entries = take_button_tables(&mut personal_doc);
        let mut personal_tables = Vec::new();
        for button in &self.buttons {
            let existing = take_entry(&mut personal_entries, button.spec.id.trim());
            let team_spec = self.team_specs.get(button.spec.id.trim());
            match (button.scope, team_spec) {
                // Only what differs from the project's button is written, so its command stays
                // the team's (and keeps needing trust) unless the user changed it.
                (Scope::Personal, Some(team_spec)) => {
                    let mut table = existing.unwrap_or_else(new_entry);
                    table.retain(|key, _| key == "id" || key == "hidden");
                    set_str(&mut table, "id", Some(button.spec.id.trim()));
                    write_override(&mut table, &button.spec, team_spec);
                    set_bool(&mut table, "hidden", button.hidden);
                    personal_tables.push(table);
                }
                (Scope::Personal, None) => {
                    let mut table = existing.unwrap_or_else(new_entry);
                    write_spec(&mut table, &button.spec);
                    set_bool(&mut table, "hidden", button.hidden);
                    personal_tables.push(table);
                }
                (Scope::Project, _) if button.hidden => {
                    let mut table = existing.unwrap_or_else(new_entry);
                    table.retain(|key, _| key == "id" || key == "hidden");
                    set_str(&mut table, "id", Some(button.spec.id.trim()));
                    set_bool(&mut table, "hidden", true);
                    personal_tables.push(table);
                }
                (Scope::Project, _) => {}
            }
        }
        // Entries the editor didn't show: overrides of project buttons that no longer exist are
        // kept (the merge ignores them); those of deleted buttons go.
        personal_tables.extend(personal_entries.into_iter().filter(|table| {
            let id = table.get("id").and_then(Item::as_str).unwrap_or_default();
            !self.team_ids.contains(id) && !ACTION_KEYS.iter().any(|key| table.contains_key(key))
        }));
        put_button_tables(&mut personal_doc, personal_tables, had_personal_buttons);

        let team = output(team_text, &team_doc);
        let personal = output(personal_text, &personal_doc);
        let final_team = team.as_deref().or(team_text);
        let final_personal = personal.as_deref().or(personal_text);
        let check = |text: Option<&str>, source| {
            text.map(|text| parse_config(text, source))
                .transpose()
                .map_err(|err| SaveError::file(err.to_string()))
        };
        let team_config = check(final_team, ConfigSource::Team)?;
        let personal_config = check(final_personal, ConfigSource::Personal)?;
        merge(team_config.as_ref(), personal_config.as_ref())
            .map_err(|err| SaveError::file(err.to_string()))?;
        Ok(SavePlan { team, personal })
    }
}

/// A lowercase id made of letters, digits and single dashes, such as `run-tests`.
pub fn slug(text: &str) -> String {
    let mut slug = String::new();
    for ch in text.trim().chars() {
        if ch.is_alphanumeric() {
            slug.extend(ch.to_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "button".to_owned()
    } else {
        slug.to_owned()
    }
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.trim().is_empty())
}

fn spec_from_entry(entry: &PartialButton) -> Option<ButtonSpec> {
    let action = match entry.action.clone()? {
        ActionSpec::Command(command) => SpecAction::Command {
            command,
            target: entry.target.unwrap_or_default(),
            cwd: entry.cwd.clone(),
            env: entry.env.clone().unwrap_or_default().into_iter().collect(),
        },
        ActionSpec::Workflow(name) => SpecAction::Workflow(name),
        ActionSpec::LaunchConfig(name) => SpecAction::LaunchConfig(name),
        ActionSpec::TabConfig(name) => SpecAction::TabConfig(name),
        ActionSpec::Url(url) => SpecAction::Url(url),
        ActionSpec::Builtin(builtin) => SpecAction::Builtin(builtin.id().to_owned()),
    };
    let placement = if entry.run_config == Some(true) {
        Placement::RunConfig
    } else if entry.overflow == Some(true) {
        Placement::Overflow
    } else {
        Placement::Toolbar
    };
    Some(ButtonSpec {
        id: entry.id.clone(),
        label: entry.label.clone(),
        icon: entry.icon.as_ref().map(|icon| match icon {
            ButtonIcon::Bundled { name, .. } => name.clone(),
            ButtonIcon::Custom(path) => path.to_string_lossy().replace('\\', "/"),
        }),
        tooltip: entry.tooltip.clone(),
        placement,
        action,
    })
}

fn parse_document(text: Option<&str>, source: ConfigSource) -> Result<DocumentMut, SaveError> {
    let Some(text) = text else {
        return Ok(DocumentMut::new());
    };
    text.parse::<DocumentMut>().map_err(|err| {
        let file = match source {
            ConfigSource::Team => ".warpium/toolbar.toml",
            ConfigSource::Personal => "the personal toolbar overrides",
        };
        SaveError::file(format!("couldn't read {file}: {err}"))
    })
}

/// Removes the `[[button]]` tables from `doc`, in file order. The comments above the first
/// table up to its last blank line are the file's header rather than that button's, so they move
/// to the document and stay at the top whatever the new order.
fn take_button_tables(doc: &mut DocumentMut) -> Vec<Table> {
    let mut tables: Vec<Table> = match doc.remove("button") {
        Some(Item::ArrayOfTables(tables)) => tables.into_iter().collect(),
        Some(_) | None => Vec::new(),
    };
    if let Some(first) = tables.first_mut() {
        let prefix = decor_prefix(first.decor());
        if let Some(blank_line) = prefix.rfind("\n\n") {
            let (header, own) = prefix.split_at(blank_line + 1);
            let document_prefix = decor_prefix(doc.decor());
            doc.decor_mut()
                .set_prefix(format!("{document_prefix}{header}"));
            first.decor_mut().set_prefix(own.to_owned());
        }
    }
    tables
}

fn decor_prefix(decor: &toml_edit::Decor) -> String {
    decor
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn take_entry(entries: &mut Vec<Table>, id: &str) -> Option<Table> {
    let index = entries
        .iter()
        .position(|table| table.get("id").and_then(Item::as_str).map(str::trim) == Some(id))?;
    Some(entries.remove(index))
}

/// Puts `tables` back as the document's `[[button]]` array, in this order. In a file that had no
/// buttons (such as a commented template), the comments stay above the new buttons.
fn put_button_tables(doc: &mut DocumentMut, mut tables: Vec<Table>, had_buttons: bool) {
    if tables.is_empty() {
        return;
    }
    if !had_buttons {
        let trailing = doc.trailing().as_str().unwrap_or_default().to_owned();
        if !trailing.trim().is_empty() {
            let prefix = decor_prefix(doc.decor());
            doc.decor_mut().set_prefix(format!("{prefix}{trailing}"));
            doc.set_trailing("");
        }
    }
    // The document prints tables by position, so the new order needs new positions.
    let base = tables.iter().filter_map(Table::position).min().unwrap_or(1);
    let mut array = ArrayOfTables::new();
    for (offset, mut table) in tables.drain(..).enumerate() {
        table.set_position(Some(base + offset as isize));
        array.push(table);
    }
    doc.insert("button", Item::ArrayOfTables(array));
}

fn new_entry() -> Table {
    let mut table = Table::new();
    table.decor_mut().set_prefix("\n");
    table
}

/// The text to write for `doc`, or `None` when nothing changed (or a missing file would be
/// created without buttons).
fn output(original: Option<&str>, doc: &DocumentMut) -> Option<String> {
    let text = doc.to_string();
    match original {
        Some(original) if original == text => None,
        Some(_) => Some(text),
        None if doc.contains_key("button") => Some(text.trim_start().to_owned()),
        None => None,
    }
}

/// Writes every field of `spec` into its `[[button]]` table, touching only the keys whose
/// values change.
fn write_spec(table: &mut Table, spec: &ButtonSpec) {
    set_str(table, "id", Some(spec.id.trim()));
    set_str(table, "label", non_empty(&spec.label));
    set_str(table, "icon", non_empty(&spec.icon));
    set_str(table, "tooltip", non_empty(&spec.tooltip));
    let (action_key, action_value) = match &spec.action {
        SpecAction::Command { command, .. } => ("command", command.as_str()),
        SpecAction::Workflow(name) => ("workflow", name.as_str()),
        SpecAction::LaunchConfig(name) => ("launch_config", name.as_str()),
        SpecAction::TabConfig(name) => ("tab_config", name.as_str()),
        SpecAction::Url(url) => ("url", url.as_str()),
        SpecAction::Builtin(id) => ("action", id.as_str()),
    };
    for key in ACTION_KEYS.iter().filter(|key| **key != action_key) {
        table.remove(key);
    }
    set_str(table, action_key, Some(action_value));
    match &spec.action {
        SpecAction::Command {
            target, cwd, env, ..
        } => {
            let keep_explicit_default = *target == CommandTarget::Current
                && table.get("target").and_then(Item::as_str) == Some("current");
            if !keep_explicit_default {
                set_str(
                    table,
                    "target",
                    (*target != CommandTarget::Current).then(|| target.as_str()),
                );
            }
            set_str(table, "cwd", non_empty(cwd));
            set_env(table, env);
        }
        SpecAction::Workflow(_)
        | SpecAction::LaunchConfig(_)
        | SpecAction::TabConfig(_)
        | SpecAction::Url(_)
        | SpecAction::Builtin(_) => {
            for key in ["target", "cwd", "env"] {
                table.remove(key);
            }
        }
    }
    set_bool(table, "run_config", spec.placement == Placement::RunConfig);
    set_bool(table, "overflow", spec.placement == Placement::Overflow);
}

/// Writes the fields of `spec` that differ from the project's `team` version of the button, as
/// a personal override.
fn write_override(table: &mut Table, spec: &ButtonSpec, team: &ButtonSpec) {
    for (key, mine, theirs) in [
        ("label", &spec.label, &team.label),
        ("icon", &spec.icon, &team.icon),
        ("tooltip", &spec.tooltip, &team.tooltip),
    ] {
        // An empty value can't unset a team field, so it is left out.
        let mine = non_empty(mine);
        set_str(table, key, mine.filter(|_| mine != non_empty(theirs)));
    }
    if spec.placement != team.placement {
        let mut set = |key: &str, value: bool| {
            table.insert(key, toml_edit::value(value));
        };
        set("run_config", spec.placement == Placement::RunConfig);
        set("overflow", spec.placement == Placement::Overflow);
    }
    if spec.action != team.action {
        let mut action_table = Table::new();
        write_spec(&mut action_table, spec);
        for (key, item) in action_table.iter() {
            if ACTION_KEYS.contains(&key) || ["target", "cwd", "env"].contains(&key) {
                table.insert(key, item.clone());
            }
        }
    }
}

/// Sets a string key (removing it for `None`), keeping a changed value's surrounding spaces and
/// trailing comment.
fn set_str(table: &mut Table, key: &str, value: Option<&str>) {
    let Some(value) = value else {
        table.remove(key);
        return;
    };
    match table.get_mut(key) {
        Some(item) if item.as_str() == Some(value) => {}
        Some(item) => replace_value(item, Value::from(value)),
        None => {
            table.insert(key, toml_edit::value(value));
        }
    }
}

/// Sets a boolean key that defaults to false: `true` is written, `false` removes it.
fn set_bool(table: &mut Table, key: &str, value: bool) {
    if !value {
        table.remove(key);
        return;
    }
    match table.get_mut(key) {
        Some(item) if item.as_bool() == Some(true) => {}
        Some(item) => replace_value(item, Value::from(true)),
        None => {
            table.insert(key, toml_edit::value(true));
        }
    }
}

fn set_env(table: &mut Table, env: &[(String, String)]) {
    if env.is_empty() {
        table.remove("env");
        return;
    }
    let unchanged = table
        .get("env")
        .and_then(Item::as_table_like)
        .is_some_and(|existing| {
            existing.len() == env.len()
                && env.iter().all(|(name, value)| {
                    existing.get(name).and_then(Item::as_str) == Some(value.as_str())
                })
        });
    if unchanged {
        return;
    }
    let mut inline = InlineTable::new();
    for (name, value) in env {
        inline.insert(name.as_str(), Value::from(value.as_str()));
    }
    match table.get_mut("env") {
        Some(item) => replace_value(item, Value::InlineTable(inline)),
        None => {
            table.insert("env", Item::Value(Value::InlineTable(inline)));
        }
    }
}

fn replace_value(item: &mut Item, mut value: Value) {
    if let Some(old) = item.as_value() {
        *value.decor_mut() = old.decor().clone();
    }
    *item = Item::Value(value);
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
