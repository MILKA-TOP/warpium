//! What the app tells the user about a check.

use warpium_upstream_check::wait_phrase;

use crate::check::{CheckError, CheckOutcome};
use crate::release::Release;
use crate::version::WarpiumVersion;

/// The newer release to offer, if `latest` is newer than `current` and the user hasn't skipped
/// it. Returns `None` when there is no current version (a development build).
pub fn update_to_offer<'a>(
    current: Option<&WarpiumVersion>,
    latest: Option<&'a Release>,
    skipped_tag: Option<&str>,
) -> Option<&'a Release> {
    let current = current?;
    let latest = latest?;
    let is_newer = latest.version().is_some_and(|version| &version > current);
    let is_skipped = skipped_tag.is_some_and(|tag| tag == latest.tag);
    (is_newer && !is_skipped).then_some(latest)
}

// Banner headings are kept short and lead with what happened: the workspace banner elides its
// text from the end to make room for its actions, and full release tags are about 30 characters.

pub const OFFER_HEADING: &str = "Warpium update available";
pub const PREPARING_HEADING: &str = "Preparing update…";
pub const READY_HEADING: &str = "Update ready";
pub const FAILED_HEADING: &str = "Couldn't update Warpium";

pub fn offer_description(current: &WarpiumVersion, release: &Release) -> String {
    let new = release_name(Some(current), release);
    let current = if new == release.tag {
        current.tag().to_owned()
    } else {
        current.short_name()
    };
    format!(
        "Version {new} (you have {current}). Update now downloads it in the background; you \
         choose when to restart."
    )
}

pub fn downloading_heading(percent: Option<u8>) -> String {
    match percent {
        Some(percent) => format!("Downloading update… {percent}%"),
        None => "Downloading update…".to_owned(),
    }
}

/// The description while the update downloads or is prepared.
pub fn in_progress_description(current: Option<&WarpiumVersion>, release: &Release) -> String {
    format!(
        "Warpium {}. Keep working; you choose when to restart.",
        release_name(current, release)
    )
}

pub fn ready_description(current: Option<&WarpiumVersion>, release: &Release) -> String {
    format!(
        "Restart now to switch to Warpium {}, or it's installed when you quit.",
        release_name(current, release)
    )
}

/// The release's short name, or its tag when that isn't a Warpium version or its short name is
/// the running version's (they leave out the upstream channel).
pub fn release_name(current: Option<&WarpiumVersion>, release: &Release) -> String {
    match release.version() {
        Some(version)
            if current.is_none_or(|current| current.short_name() != version.short_name()) =>
        {
            version.short_name()
        }
        Some(_) | None => release.tag.clone(),
    }
}

/// The result of a manual check, phrased for a toast.
pub fn manual_report(current: &WarpiumVersion, outcome: &CheckOutcome, now: u64) -> String {
    match outcome {
        CheckOutcome::Checked(Some(release))
            if release.version().is_some_and(|version| &version > current) =>
        {
            format!("Warpium {} is available (you have {current}).", release.tag)
        }
        CheckOutcome::Checked(_) => format!("Warpium is up to date ({current})."),
        CheckOutcome::Failed(CheckError::RateLimited { until })
        | CheckOutcome::Deferred { until } => format!(
            "GitHub's rate limit was reached; try again in {}.",
            wait_phrase(until.saturating_sub(now))
        ),
        CheckOutcome::Failed(err) => format!("Couldn't check for Warpium updates: {err}."),
    }
}

/// Download progress as a whole percentage, or `None` when the total size is unknown.
pub fn progress_percent(received: u64, total: u64) -> Option<u8> {
    (total > 0).then(|| (received.min(total) * 100 / total) as u8)
}

#[cfg(test)]
#[path = "notice_tests.rs"]
mod tests;
