use crate::layout::RestoreHint;

/// The panes docked in one tab's bottom panel, in sub-tab order, and which of them is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BottomDock<Id> {
    entries: Vec<DockedPane<Id>>,
    active: Option<Id>,
    /// Whether the panel is shown; an empty panel is never shown.
    open: bool,
}

/// A docked pane and where to put it back when it returns to the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DockedPane<Id> {
    pub id: Id,
    /// The layout position the pane had when it was docked, if it had a neighbor.
    pub restore_hint: Option<RestoreHint<Id>>,
}

impl<Id> Default for BottomDock<Id> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            active: None,
            open: false,
        }
    }
}

impl<Id: Copy + Eq> BottomDock<Id> {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn contains(&self, id: Id) -> bool {
        self.entries.iter().any(|entry| entry.id == id)
    }

    /// The docked pane ids in sub-tab order.
    pub fn ids(&self) -> impl Iterator<Item = Id> + '_ {
        self.entries.iter().map(|entry| entry.id)
    }

    /// The selected sub-tab, which the panel shows while it is open.
    pub fn active(&self) -> Option<Id> {
        self.active
    }

    pub fn is_open(&self) -> bool {
        self.open && !self.entries.is_empty()
    }

    /// The pane the panel currently shows, if it is open.
    pub fn visible_pane(&self) -> Option<Id> {
        self.active.filter(|_| self.is_open())
    }

    /// Adds `id` as the last sub-tab (or keeps its place if it is already docked), selects it and
    /// opens the panel.
    pub fn dock(&mut self, id: Id, restore_hint: Option<RestoreHint<Id>>) {
        match self.entries.iter_mut().find(|entry| entry.id == id) {
            Some(entry) => entry.restore_hint = restore_hint,
            None => self.entries.push(DockedPane { id, restore_hint }),
        }
        self.active = Some(id);
        self.open = true;
    }

    /// Removes `id` from the panel. When it was the selected sub-tab, the next one (or else the
    /// previous one) is selected; the panel closes once it is empty.
    pub fn undock(&mut self, id: Id) -> Option<DockedPane<Id>> {
        let index = self.entries.iter().position(|entry| entry.id == id)?;
        let removed = self.entries.remove(index);
        if self.active == Some(id) {
            self.active = self
                .entries
                .get(index)
                .or_else(|| index.checked_sub(1).and_then(|prev| self.entries.get(prev)))
                .map(|entry| entry.id);
        }
        if self.entries.is_empty() {
            self.open = false;
        }
        Some(removed)
    }

    /// Selects the sub-tab of `id` and opens the panel. Returns false if `id` isn't docked.
    pub fn select(&mut self, id: Id) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.active = Some(id);
        self.open = true;
        true
    }

    /// Shows or hides the panel. An empty panel stays hidden.
    pub fn set_open(&mut self, open: bool) {
        self.open = open && !self.entries.is_empty();
    }
}

#[cfg(test)]
#[path = "dock_tests.rs"]
mod tests;
