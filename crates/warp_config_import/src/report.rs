/// What one import run did (or, in a dry run, would do).
///
/// The item lists hold keys such as `themes/dracula.yaml` or `appearance.text.font_size`; they are
/// user-chosen names, so keep them out of release logs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Display name of the Warp installation imported from.
    pub source: Option<String>,
    pub dry_run: bool,
    /// Items that didn't exist in Warpium and were copied over.
    pub imported: Vec<String>,
    /// Items Warpium already had that were replaced with Warp's newer version.
    pub updated: Vec<String>,
    /// Items reset in Warpium because they were removed in Warp.
    pub removed: Vec<String>,
    /// Items changed on both sides; Warpium's version was kept.
    pub conflicts: Vec<String>,
    /// Items deliberately not imported, with the reason.
    pub skipped: Vec<(String, String)>,
    /// Items that failed to import, with the error.
    pub errors: Vec<(String, String)>,
    /// Number of items that were already in sync or only changed in Warpium.
    pub unchanged: usize,
}

impl ImportReport {
    /// Appends the items of `other`, a report of one phase of the same run.
    pub fn absorb(&mut self, other: ImportReport) {
        self.imported.extend(other.imported);
        self.updated.extend(other.updated);
        self.removed.extend(other.removed);
        self.conflicts.extend(other.conflicts);
        self.skipped.extend(other.skipped);
        self.errors.extend(other.errors);
        self.unchanged += other.unchanged;
    }

    pub fn changed_count(&self) -> usize {
        self.imported.len() + self.updated.len() + self.removed.len()
    }

    /// A one-line, count-only summary suitable for a toast or a release log.
    pub fn summary(&self) -> String {
        let Some(source) = &self.source else {
            return "No Warp installation found to import from".to_owned();
        };
        let mut parts = vec![
            format!("{} imported", self.imported.len()),
            format!("{} updated", self.updated.len()),
        ];
        if !self.removed.is_empty() {
            parts.push(format!("{} reset", self.removed.len()));
        }
        parts.push(format!("{} skipped", self.skipped.len()));
        if !self.conflicts.is_empty() {
            parts.push(format!(
                "{} kept Warpium's version (changed in both)",
                self.conflicts.len()
            ));
        }
        if !self.errors.is_empty() {
            parts.push(format!("{} failed", self.errors.len()));
        }
        let verb = if self.dry_run {
            "Dry run from"
        } else {
            "Synced from"
        };
        format!("{verb} {source}: {}", parts.join(", "))
    }
}
