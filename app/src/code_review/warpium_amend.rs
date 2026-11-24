//! Amend-commit helpers (design doc §5.7): the intent buttons' labels while
//! amending, and whether confirming needs the pushed-history warning first.

/// The intent button label for the current amend state: amending replaces the
/// "Commit" prefix with "Amend" ("Commit and push" -> "Amend and push").
pub(crate) fn amend_intent_label(base_label: &str, amend: bool) -> String {
    if amend {
        let rest = base_label.strip_prefix("Commit").unwrap_or(base_label);
        format!("Amend{rest}")
    } else {
        base_label.to_string()
    }
}

/// Whether confirming an amend must first warn that the previous commit is
/// already pushed (JetBrains warns before rewriting published history). The
/// warning is shown once; the second confirm proceeds.
pub(crate) fn needs_pushed_amend_warning(amend: bool, head_pushed: bool, acknowledged: bool) -> bool {
    amend && head_pushed && !acknowledged
}

#[cfg(test)]
#[path = "warpium_amend_tests.rs"]
mod tests;
