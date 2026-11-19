use serde::{Deserialize, Serialize};

use crate::layout::RestoreHint;

/// What put a pane in the panel, so later runs of the same thing can find its sub-tab again.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockOrigin {
    /// Moved or opened there by the user.
    #[default]
    Manual,
    /// Runs a command on request: a project toolbar button or an Android run. `key` identifies
    /// the requester (such as `toolbar:<button id>`), so its next run can reuse the sub-tab.
    Command { key: String },
    /// The Logcat pane.
    Logcat,
}

/// The panes docked in one tab's bottom panel, in sub-tab order, which of them is selected and
/// how the panel shows: expanded (the selected sub-tab's pane is visible), or collapsed to a strip
/// of chips. An empty panel shows nothing at all.
#[derive(Debug, Clone, PartialEq)]
pub struct BottomDock<Id> {
    entries: Vec<DockedPane<Id>>,
    active: Option<Id>,
    expanded: bool,
    /// Whether the expanded panel takes the whole tab, hiding the layout.
    maximized: bool,
}

/// A docked pane: where it goes back in the layout, what docked it, its sub-tab title and how much
/// of its output the user has seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockedPane<Id> {
    pub id: Id,
    /// The layout position the pane had when it was docked, if it had a neighbor.
    pub restore_hint: Option<RestoreHint<Id>>,
    pub origin: DockOrigin,
    /// Shown instead of the pane's own title (a toolbar button's label, say).
    pub title: Option<String>,
    /// The pane's output generation when it was last visible.
    seen_generation: u64,
}

impl<Id> DockedPane<Id> {
    /// A pane whose output up to `generation` counts as seen.
    pub fn new(id: Id, origin: DockOrigin, generation: u64) -> Self {
        Self {
            id,
            restore_hint: None,
            origin,
            title: None,
            seen_generation: generation,
        }
    }

    pub fn with_restore_hint(mut self, restore_hint: Option<RestoreHint<Id>>) -> Self {
        self.restore_hint = restore_hint;
        self
    }

    pub fn with_title(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }
}

impl<Id> Default for BottomDock<Id> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            active: None,
            expanded: false,
            maximized: false,
        }
    }
}

impl<Id: Copy + Eq> BottomDock<Id> {
    /// A panel rebuilt from a saved session. `entries` follows the saved sub-tab order, with
    /// `None` for the panes that couldn't be restored; `active` indexes into it. When the saved
    /// active sub-tab is gone, the next one (or else the previous one) is selected.
    pub fn from_restored(
        entries: Vec<Option<DockedPane<Id>>>,
        active: Option<usize>,
        expanded: bool,
        maximized: bool,
    ) -> Self {
        let active_id = active.and_then(|index| {
            let after = entries.iter().skip(index).flatten().next();
            let before = || entries.iter().take(index).flatten().last();
            after.or_else(before).map(|entry| entry.id)
        });
        let entries: Vec<_> = entries.into_iter().flatten().collect();
        let active = active_id.or_else(|| entries.first().map(|entry| entry.id));
        let has_entries = !entries.is_empty();
        Self {
            entries,
            active,
            expanded: expanded && has_entries,
            maximized: maximized && expanded && has_entries,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn contains(&self, id: Id) -> bool {
        self.entry(id).is_some()
    }

    pub fn entry(&self, id: Id) -> Option<&DockedPane<Id>> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// The docked panes in sub-tab order.
    pub fn entries(&self) -> &[DockedPane<Id>] {
        &self.entries
    }

    /// The docked pane ids in sub-tab order.
    pub fn ids(&self) -> impl Iterator<Item = Id> + '_ {
        self.entries.iter().map(|entry| entry.id)
    }

    /// The position of `id` in sub-tab order.
    pub fn index_of(&self, id: Id) -> Option<usize> {
        self.entries.iter().position(|entry| entry.id == id)
    }

    /// The selected sub-tab, which the panel shows while it is expanded.
    pub fn active(&self) -> Option<Id> {
        self.active
    }

    /// Whether the panel shows its selected pane.
    pub fn is_expanded(&self) -> bool {
        self.expanded && !self.entries.is_empty()
    }

    /// Whether the panel shows only its strip of chips.
    pub fn is_collapsed(&self) -> bool {
        !self.expanded && !self.entries.is_empty()
    }

    /// Whether the expanded panel takes the whole tab.
    pub fn is_maximized(&self) -> bool {
        self.maximized && self.is_expanded()
    }

    /// The pane the panel currently shows, if it is expanded.
    pub fn visible_pane(&self) -> Option<Id> {
        self.active.filter(|_| self.is_expanded())
    }

    /// Adds a pane as the last sub-tab (an already docked one keeps its place and seen output but
    /// takes the new hint, origin and title), selects it and expands the panel.
    pub fn dock(&mut self, docked: DockedPane<Id>) {
        let id = docked.id;
        match self.entries.iter_mut().find(|entry| entry.id == id) {
            Some(entry) => {
                entry.restore_hint = docked.restore_hint;
                entry.origin = docked.origin;
                entry.title = docked.title;
            }
            None => self.entries.push(docked),
        }
        self.active = Some(id);
        self.expanded = true;
    }

    /// Removes `id` from the panel. When it was the selected sub-tab, the next one (or else the
    /// previous one) is selected; an emptied panel resets to collapsed.
    pub fn undock(&mut self, id: Id) -> Option<DockedPane<Id>> {
        let index = self.index_of(id)?;
        let removed = self.entries.remove(index);
        if self.active == Some(id) {
            self.active = self
                .entries
                .get(index)
                .or_else(|| index.checked_sub(1).and_then(|prev| self.entries.get(prev)))
                .map(|entry| entry.id);
        }
        if self.entries.is_empty() {
            self.expanded = false;
            self.maximized = false;
        }
        Some(removed)
    }

    /// Selects the sub-tab of `id` and expands the panel. Returns false if `id` isn't docked.
    pub fn select(&mut self, id: Id) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.active = Some(id);
        self.expanded = true;
        true
    }

    /// Selects the sub-tab after the selected one (wrapping around) and expands the panel.
    pub fn select_next(&mut self) -> Option<Id> {
        self.select_relative(1)
    }

    /// Selects the sub-tab before the selected one (wrapping around) and expands the panel.
    pub fn select_previous(&mut self) -> Option<Id> {
        self.select_relative(-1)
    }

    fn select_relative(&mut self, step: isize) -> Option<Id> {
        let len = self.entries.len() as isize;
        if len == 0 {
            return None;
        }
        let current = self.active.and_then(|id| self.index_of(id)).unwrap_or(0) as isize;
        let next = (current + step).rem_euclid(len) as usize;
        let id = self.entries[next].id;
        self.select(id);
        Some(id)
    }

    /// Expands the panel or collapses it to its strip; collapsing also ends maximizing. An empty
    /// panel stays as it is.
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded && !self.entries.is_empty();
        if !self.expanded {
            self.maximized = false;
        }
    }

    /// Maximizes the panel (expanding it) or restores its height. Returns whether it is now
    /// maximized.
    pub fn toggle_maximized(&mut self) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        self.maximized = !self.is_maximized();
        self.expanded = true;
        self.maximized
    }

    /// Moves the sub-tab of `id` next to the one of `target`: after it when `after`, else before.
    /// Returns whether the order changed.
    pub fn move_entry(&mut self, id: Id, target: Id, after: bool) -> bool {
        if id == target || !self.contains(target) {
            return false;
        }
        let Some(from) = self.index_of(id) else {
            return false;
        };
        let entry = self.entries.remove(from);
        let target_index = self.index_of(target).expect("target is still docked");
        let to = if after { target_index + 1 } else { target_index };
        self.entries.insert(to, entry);
        from != to
    }

    /// The first sub-tab with `origin` that `is_busy` says is free to take a new run.
    pub fn idle_entry_with_origin(
        &self,
        origin: &DockOrigin,
        is_busy: impl Fn(Id) -> bool,
    ) -> Option<Id> {
        self.entries
            .iter()
            .filter(|entry| entry.origin == *origin)
            .map(|entry| entry.id)
            .find(|id| !is_busy(*id))
    }

    /// The title for a new sub-tab of `origin` labelled `label`: the label itself for the first
    /// one, then "label (2)", "label (3)", … while the earlier ones are busy.
    pub fn title_for_new_entry(&self, origin: &DockOrigin, label: &str) -> String {
        let existing = self
            .entries
            .iter()
            .filter(|entry| entry.origin == *origin)
            .count();
        if existing == 0 {
            label.to_owned()
        } else {
            format!("{label} ({})", existing + 1)
        }
    }

    /// Records that the user has seen the output of `id` up to `generation`.
    pub fn mark_seen(&mut self, id: Id, generation: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.seen_generation = entry.seen_generation.max(generation);
        }
    }

    /// Whether `id` has output the user hasn't seen: it isn't visible and its output generation
    /// moved on since it last was.
    pub fn has_unread(&self, id: Id, generation: u64) -> bool {
        self.visible_pane() != Some(id)
            && self
                .entry(id)
                .is_some_and(|entry| generation > entry.seen_generation)
    }
}

#[cfg(test)]
#[path = "dock_tests.rs"]
mod tests;
