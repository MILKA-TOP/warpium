//! The toolbar layout: the run-configurations dropdown and the inline buttons with their
//! overflow menu.

use crate::config::{ButtonAction, ToolbarButton};

/// What a run-configurations entry runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunConfigKind {
    /// A button with `run_config = true`, by id.
    Button(String),
    /// A launch configuration that opens sessions in the project, by name.
    LaunchConfig(String),
    /// An Android application module to build, install and start, by Gradle path (`:app`).
    AndroidApp(String),
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

/// The run-configurations entry for an Android application module, labelled `name`.
pub fn android_app_run_config(gradle_path: &str, name: &str) -> RunConfigItem {
    RunConfigItem {
        key: format!("android:{gradle_path}"),
        label: name.to_owned(),
        kind: RunConfigKind::AndroidApp(gradle_path.to_owned()),
    }
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

/// The buttons shown on the toolbar itself, in order: those not in the run-configurations
/// dropdown.
pub fn toolbar_buttons(buttons: &[ToolbarButton]) -> Vec<&ToolbarButton> {
    buttons.iter().filter(|button| !button.run_config).collect()
}

/// How the toolbar buttons fit in the space the title bar leaves for the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineFit {
    /// How many leading buttons are shown inline; the rest go to the overflow menu.
    pub visible: usize,
    /// Whether the overflow menu button is shown.
    pub overflow: bool,
}

/// Decides how many toolbar buttons fit in `available` width, given each button's width, the
/// overflow menu button's width and the width of what always follows the buttons (the
/// run-configurations widget). When every button fits there is no overflow menu; otherwise as
/// many leading buttons as fit next to the overflow menu button stay inline, in order.
pub fn fit_inline_buttons(
    button_widths: &[f32],
    overflow_width: f32,
    trailing_width: f32,
    available: f32,
) -> InlineFit {
    // Layout widths are fractional; don't collapse a button over rounding noise.
    const TOLERANCE: f32 = 0.5;
    let all = button_widths.iter().sum::<f32>() + trailing_width;
    if button_widths.is_empty() || all <= available + TOLERANCE {
        return InlineFit {
            visible: button_widths.len(),
            overflow: false,
        };
    }
    let budget = available - trailing_width - overflow_width + TOLERANCE;
    let mut used = 0.;
    let visible = button_widths
        .iter()
        .take_while(|width| {
            used += **width;
            used <= budget
        })
        .count();
    InlineFit {
        visible,
        overflow: true,
    }
}

#[cfg(test)]
#[path = "run_configs_tests.rs"]
mod tests;
