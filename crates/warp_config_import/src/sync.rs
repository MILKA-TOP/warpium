use sha2::{Digest, Sha256};

/// What one-way sync from Warp should do with one item, given the last imported version (the
/// base), Warp's current version and Warpium's current version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDecision {
    /// Warp and Warpium already agree.
    InSync,
    /// The item exists only in Warp and was never imported.
    Create,
    /// Warp changed since the last import and Warpium did not.
    Update,
    /// First import: Warpium has its own version that was never imported. Warp's version wins and
    /// Warpium's is backed up.
    Overwrite,
    /// Warp removed the item and Warpium still has the last imported version.
    Remove,
    /// Warp has nothing new: either only Warpium changed since the last import, or the item never
    /// came from Warp.
    KeepLocal,
    /// Both sides changed since the last import. Warpium's version is kept.
    Conflict,
}

impl SyncDecision {
    /// Whether applying this decision writes Warp's version into Warpium.
    pub fn takes_source(self) -> bool {
        match self {
            SyncDecision::Create | SyncDecision::Update | SyncDecision::Overwrite => true,
            SyncDecision::InSync
            | SyncDecision::Remove
            | SyncDecision::KeepLocal
            | SyncDecision::Conflict => false,
        }
    }
}

/// Applies the 3-way rule. Each argument is a content hash; `None` means the item is absent.
///
/// `initial_import` is true on the very first import, when there is no manifest yet: Warp's
/// configuration then wins over whatever Warpium had, so a fresh Warpium picks up everything.
pub fn decide(
    base: Option<&str>,
    source: Option<&str>,
    target: Option<&str>,
    initial_import: bool,
) -> SyncDecision {
    let Some(source) = source else {
        return match (base, target) {
            (Some(base), Some(target)) if base == target => SyncDecision::Remove,
            _ => SyncDecision::KeepLocal,
        };
    };
    if target == Some(source) {
        return SyncDecision::InSync;
    }
    match (base, target) {
        (None, None) => SyncDecision::Create,
        (None, Some(_)) if initial_import => SyncDecision::Overwrite,
        (None, Some(_)) => SyncDecision::Conflict,
        (Some(base), _) if base == source => SyncDecision::KeepLocal,
        (Some(base), Some(target)) if base == target => SyncDecision::Update,
        // Warpium deleted (or reset) the item while Warp changed it.
        (Some(_), None) => SyncDecision::Conflict,
        (Some(_), Some(_)) => SyncDecision::Conflict,
    }
}

/// Stable content hash used in the manifest.
pub fn content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
