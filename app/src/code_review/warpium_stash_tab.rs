//! The Code Review panel's Stashes sub-tab (design doc §5.6), JetBrains' Git
//! tool window Stashes node brought into the panel: a list of `stash@{n}`
//! entries with create/apply/pop/drop, and a read-only per-file patch viewer.
//! All git runs through `warpium_git::stash` off the main thread.

use crate::view_components::action_button::ActionButton;
use warpui::ViewHandle;
use warpui::elements::MouseStateHandle;

/// Which sub-tab the panel shows, persisted as `warpium.code_review_panel_tab`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelTab {
    #[default]
    Changes,
    Stashes,
}

impl PanelTab {
    pub fn label(self) -> &'static str {
        match self {
            Self::Changes => "Changes",
            Self::Stashes => "Stashes",
        }
    }

    pub fn from_settings(value: &str) -> Self {
        match value {
            "stashes" => Self::Stashes,
            _ => Self::Changes,
        }
    }

    pub fn as_settings_value(self) -> &'static str {
        match self {
            Self::Changes => "changes",
            Self::Stashes => "stashes",
        }
    }
}

/// Viewless per-row mouse states for the stash list (rebuilt with the list).
#[derive(Clone)]
pub(crate) struct StashRowInteraction {
    pub row: MouseStateHandle,
    pub apply: MouseStateHandle,
    pub pop: MouseStateHandle,
    pub drop: MouseStateHandle,
}

/// The stash sub-tab's state, held by the code review view.
pub(crate) struct WarpiumStashTabState {
    pub entries: Vec<warpium_git::stash::StashEntry>,
    pub loading: bool,
    pub load_error: Option<String>,
    /// `n` of the selected `stash@{n}`.
    pub selected_stash: Option<usize>,
    pub files: Vec<warpium_git::stash::StashFilePatch>,
    pub loading_patch: bool,
    pub selected_file: Option<usize>,
    /// Label of the git operation currently running, if any.
    pub busy: Option<String>,
    /// Drop asks for confirmation first, like JetBrains ("Drop 1 stash?").
    pub confirm_drop: Option<usize>,
    pub create_dialog_open: bool,
    pub row_interactions: Vec<StashRowInteraction>,
    /// Apply/pop/drop buttons per entry, parallel to `entries` (created with
    /// the list by the owning view).
    pub row_buttons: Vec<(
        ViewHandle<ActionButton>,
        ViewHandle<ActionButton>,
        ViewHandle<ActionButton>,
    )>,
}

impl Default for WarpiumStashTabState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            loading: false,
            load_error: None,
            selected_stash: None,
            files: Vec::new(),
            loading_patch: false,
            selected_file: None,
            busy: None,
            confirm_drop: None,
            create_dialog_open: false,
            row_interactions: Vec::new(),
            row_buttons: Vec::new(),
        }
    }
}

impl WarpiumStashTabState {
    /// Replaces the list and rebuilds the per-row mouse states with it.
    pub(crate) fn set_entries(&mut self, entries: Vec<warpium_git::stash::StashEntry>) {
        self.row_interactions = entries
            .iter()
            .map(|_| StashRowInteraction {
                row: MouseStateHandle::default(),
                apply: MouseStateHandle::default(),
                pop: MouseStateHandle::default(),
                drop: MouseStateHandle::default(),
            })
            .collect();
        self.entries = entries;
    }

    pub(crate) fn set_row_buttons(
        &mut self,
        row_buttons: Vec<(
            ViewHandle<ActionButton>,
            ViewHandle<ActionButton>,
            ViewHandle<ActionButton>,
        )>,
    ) {
        self.row_buttons = row_buttons;
    }

    pub(crate) fn reset(&mut self) {
        self.entries = Vec::new();
        self.row_interactions = Vec::new();
        self.row_buttons = Vec::new();
        self.load_error = None;
        self.selected_stash = None;
        self.files = Vec::new();
        self.selected_file = None;
        self.confirm_drop = None;
    }
}

#[cfg(test)]
#[path = "warpium_stash_tab_tests.rs"]
mod tests;
