//! The toolbar layout: the run-configurations dropdown and the inline buttons with their
//! overflow menu.

use crate::config::{ButtonAction, ToolbarButton};

/// Inline toolbar buttons before the rest move to the overflow menu.
pub const MAX_INLINE_BUTTONS: usize = 5;

/// What a run-configurations entry runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunConfigKind {
    /// A button with `run_config = true`, by id.
    Button(String),
    /// A launch configuration that opens sessions in the project, by name.
    LaunchConfig(String),
}

/// One entry of the run-configurations dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfigItem {
    /// Stable across reloads; remembered as the selection.
    pub key: String,
    pub label: String,
    pub kind: RunConfigKind,
}

/// The run-configurations entries: buttons marked `run_config`, then the project's launch
/// configurations that no such button already opens.
pub fn run_config_items(
    buttons: &[ToolbarButton],
    project_launch_configs: &[String],
) -> Vec<RunConfigItem> {
    let mut items = buttons
        .iter()
        .filter(|button| button.run_config)
        .map(|button| RunConfigItem {
            key: format!("button:{}", button.id),
            label: button.display_label().to_owned(),
            kind: RunConfigKind::Button(button.id.clone()),
        })
        .collect::<Vec<_>>();
    for name in project_launch_configs {
        let opened_by_button = buttons.iter().any(|button| {
            button.run_config
                && matches!(&button.action, ButtonAction::LaunchConfig(opened) if opened == name)
        });
        let key = format!("launch:{name}");
        if !opened_by_button && !items.iter().any(|item| item.key == key) {
            items.push(RunConfigItem {
                key,
                label: name.clone(),
                kind: RunConfigKind::LaunchConfig(name.clone()),
            });
        }
    }
    items
}

/// The selected entry: the remembered one if it still exists, else the first.
pub fn selected_run_config<'a>(
    items: &'a [RunConfigItem],
    remembered_key: Option<&str>,
) -> Option<&'a RunConfigItem> {
    remembered_key
        .and_then(|key| items.iter().find(|item| item.key == key))
        .or_else(|| items.first())
}

/// The buttons shown on the toolbar itself.
#[derive(Debug, PartialEq, Eq)]
pub struct ToolbarStrip<'a> {
    pub inline: Vec<&'a ToolbarButton>,
    pub overflow: Vec<&'a ToolbarButton>,
}

/// Splits the toolbar buttons (those not in the run-configurations dropdown) into the inline
/// ones and the overflow menu. When the overflow menu would hold a single button, that button is
/// shown inline instead, since the menu button would take its place anyway.
pub fn toolbar_strip(buttons: &[ToolbarButton]) -> ToolbarStrip<'_> {
    let mut inline = buttons
        .iter()
        .filter(|button| !button.run_config)
        .collect::<Vec<_>>();
    let overflow = if inline.len() > MAX_INLINE_BUTTONS + 1 {
        inline.split_off(MAX_INLINE_BUTTONS)
    } else {
        Vec::new()
    };
    ToolbarStrip { inline, overflow }
}

#[cfg(test)]
#[path = "run_configs_tests.rs"]
mod tests;
