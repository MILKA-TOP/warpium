//! Warpium: the Android Studio–style project search on cmd-O.
//!
//! In a Kotlin/Java project the Files palette gets tabs: **All**, **Classes** and **Symbols**
//! from the code index, and **Files**. This crate holds the UI-free parts: the tabs and their
//! order, how the All tab merges the other tabs' results into capped sections, and whether a
//! project gets the tabs at all. The app runs the searches and renders the rows.

/// A tab of the project search, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SearchTab {
    All,
    Classes,
    Symbols,
    Files,
}

impl SearchTab {
    pub const ALL: [SearchTab; 4] = [
        SearchTab::All,
        SearchTab::Classes,
        SearchTab::Symbols,
        SearchTab::Files,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SearchTab::All => "All",
            SearchTab::Classes => "Classes",
            SearchTab::Symbols => "Symbols",
            SearchTab::Files => "Files",
        }
    }

    /// The tab to the right, wrapping around to the first.
    pub fn next(self) -> SearchTab {
        let index = self.index();
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    /// The tab to the left, wrapping around to the last.
    pub fn previous(self) -> SearchTab {
        let index = self.index();
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    fn index(self) -> usize {
        match self {
            SearchTab::All => 0,
            SearchTab::Classes => 1,
            SearchTab::Symbols => 2,
            SearchTab::Files => 3,
        }
    }
}

/// A group of rows on the All tab, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Classes,
    Symbols,
    Files,
}

impl Section {
    pub fn label(self) -> &'static str {
        self.tab().label()
    }

    /// The tab that lists the whole section.
    pub fn tab(self) -> SearchTab {
        match self {
            Section::Classes => SearchTab::Classes,
            Section::Symbols => SearchTab::Symbols,
            Section::Files => SearchTab::Files,
        }
    }
}

/// Rows the All tab shows per section; the section's own tab lists the rest.
pub const SECTION_LIMIT: usize = 8;

/// One tab's results, best first, as input to [`merge_sections`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionResults<T> {
    pub section: Section,
    pub items: Vec<T>,
    /// The search stopped at its result limit, so there are more matches than `items`.
    pub truncated: bool,
}

/// A row of the All tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry<T> {
    /// The heading over a section, such as `Classes` or `Classes · 8 of 37`.
    Header {
        section: Section,
        label: String,
    },
    Item(T),
}

/// The All tab's rows: each non-empty section under its heading, at most `limit` rows each, in
/// the order given.
pub fn merge_sections<T>(sections: Vec<SectionResults<T>>, limit: usize) -> Vec<Entry<T>> {
    let mut entries = Vec::new();
    for SectionResults {
        section,
        items,
        truncated,
    } in sections
    {
        if items.is_empty() {
            continue;
        }
        let total = items.len();
        entries.push(Entry::Header {
            section,
            label: header_label(section, total.min(limit), total, truncated),
        });
        entries.extend(items.into_iter().take(limit).map(Entry::Item));
    }
    entries
}

/// `Classes` when every match is shown, else `Classes · 8 of 37` (`8 of 100+` when the search
/// itself was cut off).
fn header_label(section: Section, shown: usize, total: usize, truncated: bool) -> String {
    let label = section.label();
    if shown == total && !truncated {
        return label.to_owned();
    }
    let plus = if truncated { "+" } else { "" };
    format!("{label} · {shown} of {total}{plus}")
}

/// What is known about a project before its code index is consulted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectSignals {
    /// The root has Gradle or Maven build files.
    pub has_build_files: bool,
    /// The project was indexed before (Go to Class/Symbol), so its snapshot exists.
    pub has_snapshot: bool,
}

/// The state of the project's code index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexStatus {
    /// Still being built (or loaded from its snapshot and rescanned).
    Indexing,
    /// Built; `files` holds the indexed source files, `None` when unknown.
    Ready { files: Option<usize> },
}

/// Whether opening the project search may start indexing the project. Only build files or an
/// earlier index say it is a Kotlin/Java project; any other folder would be crawled for nothing.
pub fn should_index(signals: ProjectSignals) -> bool {
    signals.has_build_files || signals.has_snapshot
}

/// Whether the project search shows the code index tabs, rather than the Files palette alone.
pub fn offers_code_tabs(signals: ProjectSignals, status: IndexStatus) -> bool {
    if !should_index(signals) {
        return false;
    }
    match status {
        // A half-built index can't tell yet; the project looked like a Kotlin/Java one.
        IndexStatus::Indexing => true,
        IndexStatus::Ready { files } => files != Some(0),
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
