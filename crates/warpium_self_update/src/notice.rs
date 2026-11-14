//! What the app tells the user about a check.

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

pub fn banner_heading(current: &WarpiumVersion, release: &Release) -> String {
    format!("Warpium {} is available (you have {current}).", release.tag)
}

pub fn banner_description() -> &'static str {
    "Install it now; Warpium restarts when it's done."
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

fn wait_phrase(seconds: u64) -> String {
    let minutes = seconds.div_ceil(60).max(1);
    match minutes {
        1 => "1 minute".to_owned(),
        m if m < 60 => format!("{m} minutes"),
        m => match m.div_ceil(60) {
            1 => "1 hour".to_owned(),
            hours => format!("{hours} hours"),
        },
    }
}

#[cfg(test)]
#[path = "notice_tests.rs"]
mod tests;
