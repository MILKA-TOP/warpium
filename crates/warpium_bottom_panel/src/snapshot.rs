//! The bottom panel's saved form, stored per tab as JSON (sqlite `panels.warpium_bottom_panel`).
//!
//! Reading is lenient: unknown fields are ignored and an entry that can't be read (a kind added by
//! a later version, say) is dropped on its own, so one bad entry never loses the whole panel.

use serde::{Deserialize, Serialize};

use crate::dock::DockOrigin;

/// One tab's bottom panel.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PanelSnapshot {
    /// The expanded panel's height in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
    #[serde(default)]
    pub expanded: bool,
    #[serde(default)]
    pub maximized: bool,
    /// Index of the selected sub-tab in `entries`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<usize>,
    #[serde(default)]
    pub entries: Vec<EntrySnapshot>,
}

/// One sub-tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntrySnapshot {
    #[serde(flatten)]
    pub kind: EntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub origin: DockOrigin,
}

/// What a sub-tab holds. Processes aren't restored: a terminal comes back with its block history
/// and working directory, and Logcat starts a fresh stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryKind {
    Terminal {
        /// The terminal pane's persistent uuid (its saved blocks are keyed by it), as hex.
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    Logcat,
}

/// The lenient reading form: entries are parsed one by one.
#[derive(Deserialize)]
struct RawPanelSnapshot {
    #[serde(default)]
    height: Option<f32>,
    #[serde(default)]
    expanded: bool,
    #[serde(default)]
    maximized: bool,
    #[serde(default)]
    active: Option<usize>,
    #[serde(default)]
    entries: Vec<serde_json::Value>,
}

impl PanelSnapshot {
    pub fn to_json(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }

    /// Reads a saved panel. Returns `None` when the JSON isn't a panel at all; entries that can't
    /// be read are skipped, and `active` is moved so it keeps pointing at the same sub-tab (or the
    /// one that took its place).
    pub fn from_json(json: &str) -> Option<Self> {
        let raw: RawPanelSnapshot = serde_json::from_str(json).ok()?;
        let mut entries = Vec::with_capacity(raw.entries.len());
        // The selected sub-tab keeps its place among the entries that could be read; when it is
        // the one dropped, the next one takes its index.
        let mut active = raw.active;
        for (index, value) in raw.entries.into_iter().enumerate() {
            match serde_json::from_value::<EntrySnapshot>(value) {
                Ok(entry) => entries.push(entry),
                Err(_) => {
                    if let Some(selected) = active.as_mut()
                        && index < *selected
                    {
                        *selected -= 1;
                    }
                }
            }
        }
        let active = active
            .filter(|_| !entries.is_empty())
            .map(|index| index.min(entries.len() - 1));
        Some(Self {
            height: raw
                .height
                .filter(|height| height.is_finite() && *height > 0.),
            expanded: raw.expanded,
            maximized: raw.maximized,
            active,
            entries,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Lowercase hex of `bytes`, for terminal uuids.
pub fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Parses lowercase or uppercase hex; `None` for odd lengths or other characters.
pub fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(hex.get(index..index + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
